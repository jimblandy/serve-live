# serve-live: static directory server with change notification

This is a web server that serves up a static directory, but also
provides [server-sent events] when files in that directory changes,
so that web pages can automatically refresh.

The served address is hard-coded as `0.0.0.0:3000`. This should be
visible both from `localhost` and from the network. The served
directory defaults to the current directory, but you can pass the path
as an argument on the command line.

To hook up the automatic refresh, I put the following code in `auto-reload.js`:

    var events = new EventSource("events");
    events.addEventListener("files-changed", (event) => {
        events.close();
        location.reload()
    })

    events.onerror = (err) => {
        console.error("server-sent event source 'event' failed: ", err);
    };

Then I load it as a module from my HTML file (say, `index.html`):

      <script type="module" src="auto-reload.js"></script>

When I visit `localhost:3000`, that serves me `index.html`, which
loads the module, which subscribes to update events at
`http://localhost:3000/events`, and arranges to reload the tab
whenever I touch anything. Done!

Type `serve-live --help` for more details.

## But `npm start` does this already.

Indeed, and much more!

I wanted to do some JavaScript experimentation, and I wanted my
browser tab to automatically refresh when I changed files. If you're
writing a React app, you can use `npx create-react-app` to set
something up that does this very nicely, and it puts you right in the
sweet spot with React, webpack, JSX, and all that jazz set up and
ready to go.

But I find it distracting to have so much preprocessing going on
between me and the browser. For example, when you say:

    import './App.css';

in your JavaScript file, I get that this somehow applies the CSS rules
in `App.css` to the components you're defining, in a way that helps
webpack understand dependencies, etc.

But what does all that actually mean?  Don't the CSS files imported by
all your modules just get concatenated? I went to look this up, but
the webpack docs referred me to a blog post about "the 'Block,
Element, Modifier' methodology" for organizing CSS rules... and I just
timed out. I don't want to criticize things I don't understand
([Chesterton's Fence] and all that), but ultimately for my purposes
the JS stack there is a distraction of a scale proportional to its
depth.

And that depth is profound indeed:

    $ npx create-react-app smoove
    ...
    Installing packages. This might take a couple of minutes.
    Installing react, react-dom, and react-scripts with cra-template...

    added 1367 packages in 26s
    ...
    Happy hacking!
    $ cd smoove
    $ du -sh .
    300M	.
    $ 

When I work on implementing the web platform itself, I want to know
exactly what's happening at the level of the content APIs the browser
exposes.  I'm not trying to build anything useful in JS, I'm trying to
understand an API's behavior. Every layer that introduces a difference
between what I wrote and what happens in the browser is a distraction
for me.

## There are tons of other crates that do this already.

Yep. But there's a saying in science: "Two months in the lab can save
you half a day at the library!"

[server-sent events]: https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events/Using_server-sent_events#event_stream_format
[Chesterton's Fence]: https://en.wikipedia.org/wiki/G._K._Chesterton#Chesterton's_fence
