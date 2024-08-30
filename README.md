---
title: README
description: the README.md file of this project
author: slonkazoid
created_at: 2024-04-18T04:15:26+03:00
---

# bingus-blog

blazingly fast markdown blog software written in rust memory safe

for bingus-blog viewers: [see original document](https://git.slonk.ing/slonk/bingus-blog)

## Features

- posts are written in markdwon and loaded at runtime, meaning you
  can write posts from anywhere and sync it with the server without headache
- RSS is supported
- the look of the blog is extremely customizable, with support for
  [custom drop-ins](CUSTOM.md) for both templates and static content
- really easy to deploy (the server is one executable file)
- blazingly fast

## TODO

- [ ] blog thumbnail and favicon
- [ ] sort asc/desc
- [ ] extend syntect options
- [ ] ^ fix syntect mutex poisoning
- [ ] better error reporting and error pages
- [ ] better tracing
- [ ] replace HashMap with HashCache once i implement [this](https://github.com/wvwwvwwv/scalable-concurrent-containers/issues/139)
- [ ] make date parsing less strict
- [ ] improve home page
- [ ] multi-language support
- [ ] add credits
- [x] be blazingly fast
- [x] 100+ MiB binary size

## Configuration

see [CONFIG.md](CONFIG.md)

## Building

this project uses nightly-only features.
make sure you have the nightly toolchain installed.

build the application with `cargo`:

```sh
cargo +nightly build --release
```

the executable will be located at `target/release/bingus-blog`.

see [BUILDING.md](BUILDING.md) for more information and detailed instructions.

## Writing Posts

posts are written in markdown. the requirements for a file to count as a post are:

1. the file must be in the root of the `posts` directory you configured
2. the file's name must end with the extension `.md`
3. the file's contents must begin with a valid [front matter](#front-matter)

this file counts as a valid post, and will show up if you just `git clone` and
`cargo r`. there is a symlink to this file from the default posts directory

## Front Matter

every post **must** begin with a **valid** front matter. else it wont be listed
in / & /posts, and when you navigate to it, you will be met with an error page.
the error page will tell you what the problem is.

full example:

```md
---
title: My first post # title of the post
description: The first post on this awesome blog! # short description of the post
author: Blubber256 # author of the post
icon: /media/first-post/icon.png # icon/thumbnail of post used in embeds
icon_alt: Picture of a computer running DOOM
color: "#00aacc" # color of post, also used in embeds
created_at: 2024-04-18T04:15:26+03:00 # date of writing, this is highly
# recommended if you are on a system which doesnt have btime (like musl),
# because this is fetched from file stats by default
#modified_at: ... # see above. this is also fetched from the filesystem
tags: # tags, or keywords, used in meta and also in the ui
    - lifestyle
---
```

only first 3 fields are required. if it can't find the other 2 fields, it will
get them from filesystem metadata. if you are on musl and you omit the
`created_at` field, it will just not show up

the dates must follow the [RFC 3339](https://datatracker.ietf.org/doc/html/rfc3339)
standard. examples of valid and invalid dates:

```diff
+ 2024-04-18T01:15:26Z      # valid
+ 2024-04-18T04:15:26+03:00 # valid (with timezone)
- 2024-04-18T04:15:26       # invalid (missing Z)
- 2024-04-18T04:15Z         # invalid (missing seconds)
-                           # everything else is also invalid
```

## Non-static Routes

- `GET /`: index page, lists posts
- `GET /posts`: returns a list of all posts with metadata in JSON format
- `GET /posts/<name>`: view a post
- `GET /posts/<name>.md`: view the raw markdown of a post
- `GET /post/*`: redirects to `/posts/*`
- `GET /feed.xml`: RSS feed

## Cache

bingus-blog caches every post retrieved and keeps it permanently in cache.
there is a toggleable cleanup task that periodically sweeps the cache to
remove dead entries, but it can still get quite big.

if cache persistence is on, the cache is (compressed &) written to disk on
shutdown, and read (& decompressed) on startup. one may opt to set the cache
location to point to a tmpfs to make it save and load quickly, but not persist
across reboots at the cost of more RAM usage.

in my testing, the compression reduced a 3.21 MB cache to 0.18 MB almost
instantly. there is basically no good reason to not have compression on,
unless you have filesystem compression already of course.

## Contributing

make sure your changes don't break firefox, chromium,text-based browsers,
and webkit support

### Feature Requests

i want this project to be a good and usable piece of software, so i implement
feature requests provided they fit the project and it's values.

most just ping me on discord with feature requests, but if your request is
non-trivial, please create an issue [here](https://git.slonk.ing/slonk/bingus-blog/issues).
