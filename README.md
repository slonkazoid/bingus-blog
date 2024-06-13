---
title: "README"
description: "the README.md file of this project"
author: "slonkazoid"
created_at: 2024-04-18T04:15:26+03:00
---

# bingus-blog

blazingly fast markdown blog software written in rust memory safe

## TODO

- [x] RSS
- [x] finish writing this document
- [x] document config
- [ ] extend syntect options
- [ ] general cleanup of code
- [ ] better error reporting and error pages
- [ ] better tracing
- [x] cache cleanup task
- [ ] ^ replace HashMap with HashCache once i implement [this](https://github.com/wvwwvwwv/scalable-concurrent-containers/issues/139)
- [x] (de)compress cache with zstd on startup/shutdown
- [ ] make date parsing less strict
- [x] make date formatting better
- [ ] date formatting respects user timezone
- [x] clean up imports and require less features
- [ ] improve home page
- [x] tags
- [ ] multi-language support
- [x] be blazingly fast
- [x] 100+ MiB binary size

## Configuration

the default configuration with comments looks like this

```toml
title = "bingus-blog"  # title of the blog
# description of the blog
description = "blazingly fast markdown blog software written in rust memory safe"
markdown_access = true # allow users to see the raw markdown of a post
                       # endpoint: /posts/<name>.md
date_format = "RFC3339" # format string used to format dates in the backend
                       # it's highly recommended to leave this as default,
                       # so the date can be formatted by the browser.
                       # format: https://docs.rs/chrono/latest/chrono/format/strftime/index.html#specifiers
js_enable = true       # enable javascript (required for above)

[rss]
enable = false         # serve an rss field under /feed.xml
                       # this may be a bit resource intensive
link = "https://..."   # public url of the blog, required if rss is enabled

[dirs]
posts = "posts"        # where posts are stored
media = "media"        # directory served under /media/

[http]
host = "0.0.0.0"       # ip to listen on
port = 3000            # port to listen on

[cache]
enable = true          # save metadata and rendered posts into RAM
                       # highly recommended, only turn off if absolutely necessary
cleanup = true         # clean cache, highly recommended
#cleanup_interval = 86400000 # clean the cache regularly instead of just at startup
                       # uncomment to enable
persistence = true     # save the cache to on shutdown and load on startup
file = "cache"         # file to save the cache to
compress = true        # compress the cache file
compression_level = 3  # zstd compression level, 3 is recommended

[render]
syntect.load_defaults = false      # include default syntect themes
syntect.themes_dir = "themes"      # directory to include themes from
syntect.theme = "Catppuccin Mocha" # theme file name (without `.tmTheme`)
```

you don't have to copy it from here, it's generated if it doesn't exist

## Usage

this project uses nightly-only features.
make sure you have the nightly toolchain installed.

build the application with `cargo`:

```sh
cargo +nightly build --release
```

the executable will be located at `target/release/bingus-blog`.

### Building for another architecture

you can use the `--target` flag in `cargo build` for this purpose

building for `aarch64-unknown-linux-musl` (for example, a Redmi 5 Plus running postmarketOS):

```sh
# install the required packages to compile and link aarch64 binaries
sudo pacman -S aarch64-linux-gnu-gcc
export CC=aarch64-linux-gnu-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=$CC
cargo +nightly build --release --target=aarch64-unknown-linux-musl
```

your executable will be located at `target/<target>/release/bingus-blog` this time.

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

example:

```md
---
title: "README"
description: "the README.md file of this project"
author: "slonkazoid"
created_at: 2024-04-18T04:15:26+03:00
#modified_at: ... # see above
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

## Routes

- `GET /`: index page, lists posts
- `GET /posts`: returns a list of all posts with metadata in JSON format
- `GET /posts/<name>`: view a post
- `GET /posts/<name>.md`: view the raw markdown of a post
- `GET /post/*`: redirects to `/posts/*`

## Cache

bingus-blog caches every post retrieved and keeps it permanently in cache.
the only way a cache entry is removed is when it's requested and it does
not exist in the filesystem. cache entries don't expire, but they get
invalidated when the mtime of the markdown file changes.

if cache persistence is on, the cache is compressed & written on shutdown,
and read & decompressed on startup. one may opt to set the cache location
to point to a tmpfs so it saves and loads really fast, but it doesn't persist
across boots, also at the cost of even more RAM usage.

the compression reduced a 3.21 MB file cache into 0.18 MB with almost instantly.
there is basically no good reason to not have compression on.
