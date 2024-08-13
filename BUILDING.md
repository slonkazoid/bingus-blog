# Building bingus-blog

this guide assumes you have git and are on linux.  
at the moment, compiling on windows is supported, but not _for windows_.

1. first, acquire _rust nightly_.  
   the recommended method is to install [rustup](https://rustup.rs/),
   and use that to get _rust nightly_. choose "customize installation",
   and set "default toolchain" to nightly to save time later, provided
   you do not need _rust stable_ for something else
2. start your favorite terminal
3. then, download the repository: `git clone https://git.slonk.ing/slonk/bingus-blog && cd bingus-blog`
4. finally, build the application: `cargo +nightly build --release`
5. your executable is `target/release/bingus-blog`, copy it to your server and
   you're done!

## Building for another architecture

you can use the `--target` flag in `cargo build` for this purpose.  
examples are for Arch Linux x86_64.

here's how to compile for `aarch64-unknown-linux-gnu`
(eg. Oracle CI Free Tier ARM VPS):

```sh
# install the required packages to compile and link aarch64 binaries
sudo pacman -S aarch64-linux-gnu-gcc
cargo +nightly build --release --target=aarch64-unknown-linux-gnu
```

your executable will be `target/aarch64-unkown-linux-gnu/release/bingus-blog`.

---

a more tricky example is building for `aarch64-unknown-linux-musl`
(eg. a Redmi 5 Plus running postmarketOS):

```sh
# there is no toolchain for aarch64-unknown-linux-musl,
# so we have to repurpose the GNU toolchain. this doesn't
# work out of the box so we have to set some environment variables
sudo pacman -S aarch64-linux-gnu-gcc
export CC=aarch64-linux-gnu-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=$CC
cargo +nightly build --release --target=aarch64-unknown-linux-musl
# the reason we had to do this is because cargo tries to use
# the same toolchain as the target's name. but we can tell it to use
# the GNU one like so.
```

your executable will be `target/aarch64-unkown-linux-musl/release/bingus-blog`.
