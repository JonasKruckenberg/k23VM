# k23-vm

Development repo of the k23 WASM VM

# Operating Systems are Broken

The job of an Operating System is to make computers usable. It does that by putting a friendly face on a computer's many
implementation details, warts, and idiosyncrasies: Abstracting the different hardware devices, managing software,
exposing an easy-to-navigate user interface and providing tools and APIs to simplify software development.

You have two audiences as an OS developer: _End-users_ with a task to accomplish, be it browsing the web, editing
spreadsheets, playing games, or scrolling TikTok. And _Developers_ who want to build applications, serve _their_ user's
needs, and grow their business. End-users are obviously pivotal, but developers are equally important: **They make the
software your end-users want.**

While both audiences' needs are quite different, one thing unites them: **They don't care about your OS**. No one wants
to mess around debugging or configuring _your_ Operating System. I must **just work**. A good OS is like a good butler:
Be helpful when needed, but get out of the way when not.

![meme](./im%20not%20thinking%20about%20you%20at%20all%20meme.jpg)

# Be helpful when needed

An OS'es job is to manage computer hardware and the software a user wants to run, all while putting a friendly face on
the whole thing for developers and users.

Or, in the more sophisticated words of Wikipedia:
"An operating system (OS) is system software that manages computer hardware and software resources, and provides common
services for computer programs." [^1]

[^1]: https://en.wikipedia.org/wiki/Operating_system#:~:text=An%20operating%20system%20(OS)%20is,common%20services%20for%20computer%20programs.

Sounds relatively simple, doesn't it?

And yet, today's Operating Systems are surprisingly bad at this.

## Manage Hardware

To give them some credit, this is what today's operating systems are pretty good at. Making productive use of a computer
requires managing its various components like CPU, Memory, and Disks and their idiosyncrasies and quirks. Operating
systems have gotten quite good at this, and the experience is mostly painless; however, a few leaks in the abstraction
remain. Users must still know their machine's architecture, CPU version, and other frankly irrelevant technical details
to use their computers correctly.

The most obvious example that comes to mind is macOS software past the apple-silicon transition, where I have
experienced firsthand non-tech-savvy friends saying, "Oh, I didn't know what to select here, Intel chip or Apple
Silicon. What even is the difference?". And this is still one of the better experiences.

## Manage Software

The OS is responsible for managing and providing the environment for running programs.
This is quite involved:

- installing, uninstalling, updating programs
- starting, stopping, scheduling programs
- allocating hardware resources (e.g. memory) to programs based on need but in a fair way
- isolation between programs

And this is where things begin to fall apart; Just the first step of getting software onto the computer is painful, from
Windows' random installers off the internet to macOS' extortionate app store pricing to Linux user's apparent favorite
pastime of manual dependency management, the experience is horrible both for users and developers.

But this is not where the trouble ends; isolation between programs is often nonexistent, and regular users have limited
control over a program's permissions and security settings. The result is what we see today: Expensive security software
and malware galore.

Ironically, even making full use of the hardware is sometimes difficult: Today's OSes target a wide variety of
computers, from little Raspberry Pis to hundred-core power workstations. Programs (including built-in OS software) face
a dilemma: Supporting all these devices means targeting the _lowest common denominator_ by default, which means most of
the power of more capable hardware is thrown away!

## Putting a Friendly Face On It

I saved this for last because this is where operating systems drop the ball the hardest: They thrust upon users a
flood of disparate interfaces, tools, and configurations without much thought.

The situation is even worse for developers: Having to spend significant time just installing prerequisites has
sadly become common, APIs are often sparsely documented and hard to use, and don't even get me started on include
paths, rpaths, and why I need a degree in advanced linker studies to build my app. And this is if you're lucky:
Some companies seem hellbent on making their operating systems as hostile for developers as possible.

TODO: summarize

# TODO

## How the OS lost the game

Applications are in a weird spot today. A decade ago, companies prided themselves on being the best
operating system for developers, having the best application development tooling, and the most welcoming ecosystem.
Native apps were king. Today, you will be treated with careless indifference at best and active hostility at worst.

At the same time, the relevance of the native app has slowly faded; installing apps through installers, documents
on a hard drive, and <TODO> gave way to the instant, painless, and <TODO> experience of the Web and cloud applications.
The popularization of tools like Electron has proven another win of the Web, its _developer experience_. No other
platform is so accessible and has such a great ecosystem of learning resources and tooling.

However, we are also witnessing this new platform reach its limits. Some websites are shipping megabytes of
JavaScript and attempting to render complex UIs with expectedly mixed results. The saying 'the web is for websites,
not fully interactive apps' often heard on social media is reductionist and discredits the engineering marvels of
modern web browsers. Yet, there is a kernel of truth in it. App developers would be happier on a platform designed for
their needs.

You might think it's weird to talk so extensively about the web in a post about operating systems, but they are
intimately intertwined. In 1995, at the browser company Netscape's IPO, Marc Andreessen (I don't like the guy either)
reportedly said that Netscape would reduce Windows to a "poorly debugged set of device drivers" because when
applications increasingly move to the web, the operating system is reduced to the thing you use to run Chrome. [^2]

[^2]: ChromeOS takes this to its logical conclusion and in a weird roundabout way, is doing the same thing as I am
   proposing, but approached from the opposite side. 
   It also perfectly highlights how the web platform is reaching its limits and why that approach is not the future.

## Can you take a look at my OS I think its broken

The issues with traditional (dare I say legacy) operating systems don't stop there though, from leaky abstractions, to
the flood disparate interfaces and the myriads of security flaws; it becomes clear the OS has it's problems. At it's
core

## The Operating System Renaissance

# A breath of fresh air

These 3 core values

- **Manage & Abstract Hardware**
- **Manage & Isolate Programs**
- **Be Great to Develop For**

## Core Philosophy

1. **Abstract Hardware** <br>
   Users shouldn't need to care about machine architecture, CPU version or other details.
   The OS abstracts the hardware, that is its job. You build your program _**once**_, and it runs everywhere k23 runs.
2. **Isolate Programs** <br>
   Users shouldn't need to worry about clicking "bad links", accidentally installing malware, or getting hacked. The OS
   isolates programs from each other, managing permissions, that is its job. k23 strongly sandboxes all programs,
   and all system APIs are built around fine-grained capability based access control.
3. **Be a Development** <br>

“Never. Yeah. In the IPO press cycle, Mark Andreessen is quoted as saying that, quote, Netscape will soon reduce Windows
to a poorly-debugged set of device drivers.

It's such a good quote. And there's so much behind it too. If you really dwell in that quote, what does it mean?

If one of the things he's saying is, Windows is a platform upon which independent software vendors write applications.
So Windows is the way that currently people write software for businesses and consumers to use. And if we are going to
reduce Windows to a poorly-debugged set of device drivers, what I'm implying is these crappy static web pages that get
served right now, that is merely a step on our journey to enabling rich web applications.
Think JavaScript, CSS, eventually Java and Flash. The web will be a way that developers write their applications. That's
right there, implicit in the quote.”

“And so when they're saying we're going to reduce Windows, blah, blah, blah, it's saying, okay, Windows has all this
stuff right now for developers. But essentially, you're going to use Windows or any operating system just to boot it up,
connect to all your peripherals and your screen and your mouse and your keyboard and everything, and you'll open your
browser and you'll do everything through the browser. And that scared the hell out of Microsoft.

Not specifically this quote, but Microsoft had come to the same conclusion, too, of, oh my god, if the web becomes the
platform of the future, all the reasons why we have all this incredible business, people feeling the need to use our
operating system to be able to get access to their favorite software and for developers to build applications on our
platform to get access to the users, that could go away. And in the same memo that you were quoting earlier, the
Internet title wave, Bill Gates famously says, and when I say famously, it's because the Department of Justice later
grabbed this quote and used it as an exhibit. Bill writes, a new competitor born on the Internet is Netscape.”

From Acquired: Microsoft Volume II, Jul 22, 2024
https://podcasts.apple.com/de/podcast/acquired/id1050462261?i=1000662929328
This material may be protected by copyright.

## Programs in k23

Programs in k23 are not compiled to machine code, but to WebAssembly (WASM) a bytecode format that is fast, secure and
portable.
k23 runs these programs in a sandboxed environment, providing them with access to the underlying hardware through a set
of *POSIX-like* interfaces.

This portable WASM bytecode is just-in-time compiled on the target machine which


[//]: # (are made up of smaller building blocks called [`Components`][component-model])


[//]: # (Programs in k23 are WASM  which are essentially a collection of)

[//]: # (dynamically linked WASM modules or components. You can think of this like one executable and potentially)

[//]: # (many dynamic libraries packaged together.)

[//]: # (WASM components - the building blocks of k23 programs - can )

[//]: # (can be written in any language that supports WASM &#40;like, C, Rust, Swift, Haskell and many more&#41;. )

[//]: # (These components interact through language-agnostic, high-level interfaces that describe available functions.)

[//]: # (Components can import other components and can likewise be imported.)

[//]: # ()

[//]: # (Consider the following simplified example &#40;written in the low-level WASM text representation WAT&#41;, that just prints the)

[//]: # (current time to STDOUT:)

[//]: # ()

[//]: # (```wat)

[//]: # (&#40;component)

[//]: # (    &#40;import "wasi:clocks/wall-clock@0.2.2" &#40;instance $time)

[//]: # (        &#40;export "now" &#40;func ...&#41;&#41;)

[//]: # (    &#41;&#41;)

[//]: # (    &#40;import "wasi:cli/stdout@0.2.2" &#40;component $stdout)

[//]: # (        &#40;export "get-stdout" &#40;func ...&#41;&#41;)

[//]: # (    &#41;&#41;)

[//]: # (    )

[//]: # (    &#40;func &#40;export "print-time"&#41;)

[//]: # (        ... transitively calls &#40;func $time "now"&#41; to get the current time)

[//]: # (        and &#40;func $stdout "get-stdout"&#41; + "output-stream.write" to write to the STDOUT)

[//]: # (    &#41;)

[//]: # (&#41;)

[//]: # (```)

[//]: # ()

[//]: # (As you can see the program imports asks the OS to provide it with implementations)

[//]: # (for the [`"wasi:clocks/wall-clock@0.2.2"`][wasi-clocks-wall-clock] and [`"wasi:cli/stdout@0.2.2"`][wasi-cli-stdout])

[//]: # (interfaces, from which it imports the `"now"` and `"get-stdout"` functions respectively.)

[//]: # (It then exports a function called `"print_time"` that will use those imports to print the current)

[//]: # (time to STDOUT.)

[//]: # ()

[//]: # (Crucially programs don't need to care *how* these imports are actually fulfilled, the OS might have)

[//]: # (builtin implementations, or fetch components from the network; As long as the API contract laid out by)

[//]: # (the interface is upheld, the OS is free to choose the most optimal approach. You can think of this as)

[//]: # ("typed dynamic linking" where you don't link against a specific library, but against an API contract.)

[//]: # ()

[//]: # (This enables a number of cool things:)

[//]: # ()

[//]: # (- **Dynamic Implementation Selection** - A program that depends on the `"wasi:filesystem` interface can work)

[//]: # (  with **any** file system implementation, so the User or OS are free to most appropriate implementation &#40;ext4, fat,)

[//]: # (  etc.&#41;.)

[//]: # (- **Language Independence** - Since the interface definitions are language-agnostic modules and components can be)

[//]: # (  composed)

[//]: # (  together regardless of the language they are written in. You can have C code calling Swift calling Haskell without any)

[//]: # (  issues.)

[//]: # (- **Bring only what you need** - Each Program comes with and explicit dependency tree)

[//]: # (## Microkernel with Dependency Management & Registry)

[//]: # ()

[//]: # (k23 is naturally designed as a microkernel. This means the kernel itself is rather lightweight, it only contains)

[//]: # (bootstrapping code, and a WASM Virtual Machine. Everything else is implemented as userspace programs, from drivers to)

[//]: # (libraries and programs.)

[//]: # ()

[//]: # (This microkernel architectures provides *much* greater security and stability since crashes or vulnerabilities in)

[//]: # (one component remain contained to that component. Aside from that the kernel itself - being much smaller - becomes)

[//]: # (easier to audit and test.)

[//]: # ()

[//]: # (In addition to these benefits shared with all microkernels k23 has builtin *first class dependency management*.)

[//]: # ()

[//]: # (Each program already explicitly declares its imports, and with these the OS will build a full dependency)

[//]: # (tree that then gets fetched from a [*first party package registry*][wasm-warg]. Programs can import )

[//]: # ()

[//]: # (- a plain name that leaves it up to the developer to "read the docs" or otherwise figure out what to supply for the import;)

[//]: # (- an interface name that is assumed to uniquely identify a higher-level semantic contract that the component is requesting an unspecified wasm or native implementation of;)

[//]: # (- a URL name that the component is requesting be resolved to a particular wasm implementation by fetching the URL.)

[//]: # (- a hash name containing a content-hash of the bytes of a particular wasm implemenentation but not specifying location of the bytes.)

[//]: # (- a locked dependency name that the component is requesting be resolved via some contextually-supplied registry to a particular wasm implementation using the given hierarchical name and version; and)

[//]: # (- an unlocked dependency name that the component is requesting be resolved via some contextually-supplied registry to one of a set of possible of wasm implementations using the given hierarchical name and version range.)

[//]: # (## Microkernel Without the Performance Issues)

[//]: # ()

[//]: # (## WASM VM Design)

### Allocation

In WASM the instances are defined in the `Store` which owns
all related resources (memories, tables, globals etc.).
The WASM specification

### Execution

[component-model]: https://github.com/WebAssembly/component-model

[wasi-clocks-wall-clock]: https://github.com/WebAssembly/wasi-clocks/blob/110b161782f4900b188d326aeb303b211e4cd9e8/wit/wall-clock.wit#L17

[wasi-cli-stdout]: https://github.com/WebAssembly/wasi-cli/blob/0ed19accf7e9e677ad5911fc14cd1af7ceba1887/wit/stdio.wit#L11

[wasm-warg]: https://github.com/bytecodealliance/registry