---
title: "README"
description: "the README.md file of this project"
author: "slonkazoid"
created_at: 2024-04-18T04:15:26+03:00
---

# bingus-blog

blazingly fast markdown blog software written in rust memory safe

## TODO

- [ ] RSS
- [x] finish writing this document
- [x] document config
- [ ] extend syntect options
- [ ] general cleanup of code
- [ ] make `compress.rs` not suck
- [ ] better error reporting and pages
- [ ] better tracing
- [ ] cache cleanup task
- [ ] (de)compress cache with zstd on startup/shutdown
- [ ] make date parsing less strict
- [ ] make date formatting better
- [ ] clean up imports and require less features
- [x] be blazingly fast
- [x] 100+ MiB binary size

## Configuration

the default configuration with comments looks like this

```toml
# main settings
host = "0.0.0.0" # ip to listen on
port = 3000 # port to listen on
title = "bingus-blog" # title of the website
description = "blazingly fast markdown blog software written in rust memory safe" # description of the website
posts_dir = "posts" # where posts are stored
markdown_access = true # allow users to see the raw markdown of a post

[cache] # cache settings
enable = true # save metadata and rendered posts into RAM
              # highly recommended, only turn off if asolutely necessary
#persistence = "..." # file to save the cache to on shutdown, and
                     # to load from on startup. uncomment to enable

[render] # post rendering settings
syntect.load_defaults = false # include default syntect themes
syntect.themes_dir = "themes" # directory to include themes from
syntect.theme = "Catppuccin Mocha" # theme file name (without `.tmTheme`)

[precompression] # precompression settings
enable = false # gzip every file in static/ on startup
watch = true # keep watching and gzip files as they change
```

you don't have to copy it from here, it's generated if it doesn't exist

## Usage

build the application with `cargo`:

```sh
cargo build --release
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
cargo build --release --target=aarch64-unknown-linux-musl
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
- 2024-04-18T04:15:26Z      # invalid (missing Z)
- 2024-04-18T04:15Z         # invalid (missing seconds)
-                           # everything else is also invalid
```

## Routes

- `GET /`: index page, lists posts
- `GET /posts`: returns a list of all posts with metadata in JSON format
- `GET /posts/<name>`: view a post
- `GET /posts/<name>.md`: view the raw markdown of a post
- `GET /post/*`: redirects to `/posts/*`
