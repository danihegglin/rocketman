# rocketman

A fast, minimal HTTP client — think Postman without the weight — built in Rust with [GPUI](https://www.gpui.rs/) (via [gpui-kit](https://gpui-kit.com)).

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

The binary lands in `target/x86_64-unknown-linux-gnu/release/rocketman`, and the script fails if it needs any glibc symbol newer than 2.17. At runtime the target machine needs `libxkbcommon-x11` and `libxcb`, plus a Vulkan driver. CI builds it the same way and checks that it loads on CentOS 7.

## Tests

```sh
cargo test
```
