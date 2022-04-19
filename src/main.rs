use anyhow::{bail, Result};
use argh::FromArgs;
use futures_util::StreamExt;
use notify::Watcher;
use warp::{Filter, Reply};
// Use warp's re-export of http crate, to be sure we get the right version.
use http::response::Response;
use http::status::StatusCode;
use warp::http;
use warp::http::Uri;
use warp::hyper::Body;

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr as _;
use std::{fs, net};

mod stream_own;

#[derive(FromArgs)]
/// Serve a directory's contents, providing server-sent events when files are changed.
struct ServeLive {
    #[argh(positional)]
    /// directory to serve. (Default: '.')
    path: Option<String>,

    #[argh(option, default = r#"arg_address("0.0.0.0:3000")"#)]
    /// address to listen for HTTP requests on. (Default: 0.0.0.0:3000)
    address: net::SocketAddr,

    /// path for server-sent events reporting file changes. (Default: 'events')
    #[argh(option, default = r#"String::from("events")"#)]
    event_path: String,
}

fn arg_address(arg: &str) -> net::SocketAddr {
    net::SocketAddr::from_str(arg).unwrap()
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args: ServeLive = argh::from_env();

    // Choose the directory subtree we'll serve from.
    let root = args
        .path
        .map(|a| Ok(PathBuf::from(a)))
        .unwrap_or_else(std::env::current_dir)?;
    if !root.is_dir() {
        bail!("Not a directory: {}", root.display());
    }
    let root = fs::canonicalize(&root)?;

    println!("Serving HTTP at {:?}", args.address);
    println!("    Serving files from {}", root.display());

    // Create a filter for server-sent events.
    let events_path = args.event_path;
    let root_clone = root.clone();
    let events = warp::path(events_path)
        .and(warp::get())
        .map(move || result_to_response("server-sent event source", serve_events(&root_clone)));

    // Create a filter for serving actual files.
    //
    // Not using warp::fs::dir because of
    // https://github.com/seanmonstar/warp/issues/953
    let base_uri = Uri::from_static("/");
    let files = warp::path::tail().map(move |tail: warp::filters::path::Tail| {
        result_to_response("file server", serve_file(tail, &base_uri, &root))
    });

    warp::serve(events.or(files)).run(args.address).await;

    Ok(())
}

/// Convert an `anyhow::Result` to a warp `Response`.
///
/// Errors are returned as: "500 Internal Server Error"
/// [[RFC7231, Section 6.6.1](https://tools.ietf.org/html/rfc7231#section-6.6.1)]
fn result_to_response<T: warp::Reply>(who: &str, result: Result<T>) -> warp::reply::Response {
    match result {
        Ok(response) => response.into_response(),
        Err(err) => {
            log::error!("error from {}: {}", who, err);
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(format!("Internal server error:\n{}", err))
                .unwrap()
                .into_response()
        }
    }
}

fn serve_events(dir: &Path) -> Result<warp::reply::Response> {
    use warp::sse;

    /// The type of server-sent `files-changed` events.
    #[derive(serde::Serialize)]
    struct FilesChanged {
        paths: Vec<PathBuf>,
        dropped: bool,
    }

    /// Return true if `path` is the name of an Emacs
    /// auto-save file.
    fn is_auto_save(path: &Path) -> bool {
        // There's no better way to do this at the moment.
        // https://github.com/rust-lang/rust/issues/49802
        path.file_name()
            .and_then(OsStr::to_str)
            .map_or(false, |s| s.starts_with(".#"))
    }

    /// Return true if `path` is the name of an Emacs
    /// backup file.
    fn is_backup(path: &Path) -> bool {
        path.to_str().map_or(false, |s| s.ends_with('~'))
    }

    fn is_git_metadata(path: &Path) -> bool {
        path.components()
            .any(|c| matches!(c, Component::Normal(s) if s == ".git"))
    }

    // Create an asynchronous channel for the `notify` watcher to send events
    // on. It's a bounded channel, so we must notify the client of any events
    // dropped due to backpressure.
    let (mut tx, rx) = futures_channel::mpsc::channel(1);
    let mut watcher = notify::recommended_watcher({
        // True if the last send failed.
        let mut dropped = false;

        move |res: notify::Result<notify::Event>| match res {
            Err(error) => {
                log::error!("error from file change monitor: {}", error);
            }
            Ok(event) => {
                log::trace!("event from file change monitor: {:?}", event);
                let mut event = FilesChanged {
                    paths: event.paths,
                    dropped,
                };

                // Ignore changes to some files.
                //
                // Ideally this would be more configurable.
                //
                // I had an impulse that we should .gitignore files, but then I
                // realized that's not the right set of files: many files that
                // you would want listed in .gitignore are computation products
                // that you do want to serve to the browser.
                event.paths.retain(|path| {
                    !is_auto_save(path) && !is_backup(path) && !is_git_metadata(path)
                });
                if event.paths.is_empty() {
                    log::trace!("    all changed filenames filtered out, event dropped");
                    return;
                }

                match serde_json::to_string(&event) {
                    Ok(json) => match tx.try_send(json) {
                        Ok(()) => {
                            dropped = false;
                        }
                        Err(error) => {
                            if error.is_full() {
                                dropped = true;
                            } else if !error.is_disconnected() {
                                log::error!("error sending on channel: {}", error);
                            }
                        }
                    },
                    Err(error) => {
                        log::error!("error serializing event: {}", error);
                    }
                }
            }
        }
    })?;

    log::trace!("created watcher");
    watcher.watch(dir, notify::RecursiveMode::Recursive)?;

    // We can now treat `rx` as a stream of JSON-ified `notify`
    // events. Convert that into a stream of `warp::sse::Event`s.
    //
    // We try to do all filtering and fallible operations in the
    // watcher's event handler above, so that this conversion never
    // has to drop messages from `rx`, allowing us to use `map`
    // in favor of `filter_map` and annoying error-catching.
    //
    // However, `sse::keep_alive` requires a stream of `Result`
    // items. Since we do not return errors, we need to spell out an
    // error type. `Infallible` is good enough.
    let event_stream = rx.map(move |data: String| -> Result<_, std::convert::Infallible> {
        let event: sse::Event = sse::Event::default().event("files-changed").data(data);
        Ok(event)
    });

    // Let this stream take ownership of `watcher`, so that if the
    // server-sent event stream ends or gets dropped, the notify
    // descriptor will get freed promptly.
    log::trace!("serving modification events for {:?}", dir);
    let event_stream = stream_own::own(event_stream, watcher);
    Ok(sse::reply(
        sse::keep_alive()
            .interval(std::time::Duration::from_secs(600))
            .stream(event_stream),
    )
    .into_response())
}

fn serve_file(tail: warp::path::Tail, base_uri: &Uri, root: &Path) -> Result<Response<Body>> {
    let query = String::new();
    let mut path = root.join(tail.as_str());

    if path.is_dir() {
        let tail = tail.as_str();
        if tail.is_empty() || tail.ends_with('/') {
            path.push("index.html");
        } else {
            // Produce a redirect to the path that does end with a slash.
            let path = format!("{}/{}", tail, query);
            let mut parts = base_uri.clone().into_parts();
            parts.path_and_query = Some(http::uri::PathAndQuery::from_maybe_shared(path)?);
            let uri = Uri::from_parts(parts)?;
            log::trace!("redirecting to URI: {:?}", uri);
            return Ok(warp::redirect(uri).into_response());
        }
    }

    let mime_type = match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("css") => Some("text/css"),
        Some("html") => Some("text/html"),
        Some("js") => Some("application/javascript"),
        Some("png") => Some("image/png"),
        _ => None,
    };

    match fs::read(&path) {
        Ok(bytes) => {
            let mut response = Response::builder().status(StatusCode::OK);
            if let Some(mime_type) = mime_type {
                response = response.header("Content-Type", mime_type);
            }
            log::trace!("serving contents of {:?}", path);
            Ok(response.body(bytes.into())?)
        }
        Err(err) => {
            log::error!("serve_file:");
            log::error!("    tail: {:?}", tail);
            log::error!("    path: {}", path.display());
            log::error!("    error: {}", err);
            Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body("request failed".into())?)
        }
    }
}
