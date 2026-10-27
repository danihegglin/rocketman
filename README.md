# rocketman

A fast, minimal HTTP client — think Postman without the weight — built in Rust with [GPUI](https://www.gpui.rs/) (via [gpui-kit](https://gpui-kit.com)), plus a terminal UI built with [ratatui](https://ratatui.rs).

Collections are plain **OpenAPI 3.x** files, so they live happily in git next to your code and work with every other OpenAPI tool.

## Run

```sh
cargo run --release              # last used folder, or the default collections folder
cargo run --release -- ./api     # use any folder of OpenAPI files (e.g. inside a repo)
```

The default folder is `~/Library/Application Support/rocketman/collections` (platform equivalent elsewhere). On first run it is seeded with an `httpbin` example collection.

## Keys

| Key | Action |
| --- | --- |
| ⌘↵ | Send |
| Esc | Cancel the in-flight request |
| ⌘S | Save the request |
| ⌘N / ⇧⌘N | New request / new collection |
| ⌘L | Focus the URL |
| ⌘I | Import an OpenAPI file as a collection |
| ⌘O | Open another folder |
| ⌘R | Reload collections from disk |

Right-click a collection or request for rename, duplicate, delete and reveal-in-Finder.

## Terminal UI

`rocketman-tui` offers the same features in a terminal: the collection sidebar with filter, the request editor (name, method, URL, path variables, query params, headers, body), sending and cancelling, the response with status, timing, size, body and headers, and new / rename / duplicate / delete / import / open folder / reload. Both frontends share the same core (`src/lib.rs`), so collections and behaviour are identical.

```sh
cargo run --release --bin rocketman-tui              # last used folder
cargo run --release --bin rocketman-tui -- ./api     # any folder of OpenAPI files
cargo install --path . --no-default-features --features tui   # terminal only, no GPUI/system graphics libraries needed
```

| Key | Action |
| --- | --- |
| Tab / Shift-Tab | Next / previous pane (sidebar, request, response) |
| F5, Ctrl-↵ or Alt-↵ (↵ in the URL too) | Send |
| Esc | Cancel the in-flight request |
| Ctrl-S | Save the request |
| Ctrl-N / Alt-N | New request / new collection |
| Ctrl-L | Focus the URL |
| Ctrl-I or Alt-I | Import an OpenAPI file as a collection |
| Ctrl-O | Open another folder |
| Ctrl-R | Reload collections from disk |
| Ctrl-Q | Quit (asks about unsaved requests) |
| F1 | All keys |

In the sidebar, `/` filters, `n` / `N` create, `d` duplicates, `r` renames, `x` deletes. In the response, `v` switches between body and headers, `y` copies (via OSC 52, works over SSH) and `s` saves the body to a file.

## How requests map to OpenAPI

Every `*.yaml`, `*.yml` or `*.json` file with an `openapi: 3.x` key is a collection; `info.title` is its name.

**Import** (⌘I, the sidebar `+` menu, or drag files onto the window) copies an existing OpenAPI 3.x file into the current folder byte-for-byte, so its comments and formatting are kept until you first save a change. Files that aren't OpenAPI 3.x (including Swagger 2.0) are rejected with a message and nothing is written. If the spec `$ref`s other files, you're warned to copy those alongside it.

| Editor | OpenAPI |
| --- | --- |
| Name | operation `summary` |
| Method + URL | path item + operation; the host part becomes `servers` (document level, or operation level when it differs) |
| `{var}` in the URL | `in: path` parameter, value in `example` |
| Query params / headers | `in: query` / `in: header` parameters, values in `example` |
| Body + content type | `requestBody.content.<type>.example` |

Saving edits the document in place: schemas, responses, descriptions, tags, `$ref`s, extensions and key order you didn't touch stay as they were. Unchanged requests round-trip byte-for-byte at the data level (see the tests in `src/openapi.rs`). Writes are atomic (temp file + rename).

Two requests with the same method and path can't exist in one OpenAPI document, so a save that would collide is refused with a message instead of overwriting.

## Design notes

- HTTP runs on its own thread (ureq, rustls); the UI thread never blocks on the network.
- JSON responses are re-indented in a single pass without parsing into values, so numbers and key order show exactly as sent. Display is capped at 8 MB (highlighting at 2 MB); **Save** writes the full body.
- Unsaved edits are kept per request while you switch around (dot in the sidebar). Closing or quitting with unsaved edits asks first.
- The sidebar is a virtualized list.

## Linux builds for older distros (glibc 2.17)

A plain `cargo build` on Linux links against the build machine's glibc, so the binary won't start on older systems. To build one that runs on glibc 2.17 and newer (CentOS/RHEL 7 onwards):

```sh
sudo apt-get install libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev libfontconfig-dev libfreetype-dev
pip install ziglang==0.13.0
cargo install cargo-zigbuild --locked
scripts/build-linux-glibc217.sh
```

The binaries land in `target/x86_64-unknown-linux-gnu/release/` (`rocketman` and `rocketman-tui`), and the script fails if either needs any glibc symbol newer than 2.17. At runtime the desktop app needs `libxkbcommon-x11` and `libxcb`, plus a Vulkan driver; the terminal app needs nothing beyond glibc. CI builds it the same way and checks that it loads on CentOS 7.

## Tests

```sh
cargo test
```

## Releasing

Bump `version` in `Cargo.toml` (run `cargo check` to update `Cargo.lock`) and merge to `main`. The Release workflow tags the commit as `v<version>` and publishes a GitHub release with generated notes. Versions with a suffix such as `0.2.0-beta.1` are marked as pre-releases. The glibc 2.17 Linux binary is built and attached to each release.
