# va — a CLI for your project

Every project accumulates commands: configure, compile, test, package, clean.
`va` gathers them into a single command-line tool for your project. You write
the commands once as named **goals** in a `vafile`, and run them with
`va <goal>`.

It follows in the footsteps of [`make`](https://www.gnu.org/software/make/)
(originally written by Stuart Feldman at Bell Labs; GNU Make today) and
[`just`](https://github.com/casey/just) (by Casey Rodarmor) — the same idea of a
project-local file full of named recipes — and owes both a great deal. `va`
keeps that core idea and makes a few different choices around how goals are named
and invoked (see [Subcommands](#subcommands)).

## Install

Download the binary for your platform from the latest release, make it
executable, and drop it on your `PATH`:

**Linux (x86_64)**

```
curl -L -o va https://github.com/Wenke-D/va/releases/latest/download/va-linux-x86_64
chmod +x va && sudo mv va /usr/local/bin/
```

**macOS (Apple Silicon)**

```
curl -L -o va https://github.com/Wenke-D/va/releases/latest/download/va-macos-arm64
chmod +x va && sudo mv va /usr/local/bin/
```

The Linux binary is statically linked (musl), so it runs on any distribution
with nothing else to install. On macOS, Gatekeeper blocks unsigned downloads —
clear the quarantine flag with `xattr -d com.apple.quarantine va` (or right-click
→ Open the first time).

Prefer to build it yourself? `cargo build --release` (or `va install`, which
builds and installs to `/usr/local/bin`).

## Features

- **Goals in a `vafile`.** Define a command once; run it as `va <goal>`.
- **Parameters.** A goal can take positional arguments: `va test MySuite`.
- **Sequencing.** A goal can depend on other goals, which run first, in order.
- **Subcommands.** Goals can be grouped into namespaces: `va docker build`.
- **Validated up front.** The whole file is checked before anything runs —
  unknown dependencies and dependency cycles are reported, not discovered
  mid-run.
- **One file, current directory.** `va` reads the `vafile` in the directory you
  run it from. No searching up the tree.

## Defining a vafile

A `vafile` is a list of goals. Each goal is a name ending in `:`, followed by an
indented body — the shell commands to run.

```
# configure the CMake build tree
configure:
    cmake -S . -B build

# compile what's already configured
compile:
    cmake --build build

# wipe the build tree
clean:
    rm -rf build
```

Run a goal by name:

```
va configure        # runs: cmake -S . -B build
va clean            # runs: rm -rf build
va                  # with no goal, lists everything available
```

A goal can take a parameter, used in the body as `{{name}}` (or `$name`):

```
test name:
    ctest --test-dir build -R {{name}}
```

```
va test MySuite     # runs: ctest --test-dir build -R MySuite
```

### How a body runs

A goal's body runs as a **single shell** (`sh`), so `cd` and variables persist
from one line to the next:

```
build:
    cd frontend
    npm install         # runs inside ./frontend
```

The body is also **fail-fast**: it runs under `set -e`, so the first command
that fails stops the goal — the same "stop on first failure" rule that applies
to dependencies. To let a body keep going after a failure, start it with
`set +e`, or ignore a single command with `cmd || true`:

```
check:
    set +e              # this goal tolerates failures
    diff a b
    echo "compared"
```

## Sequencing goals

A goal can list other goals after its `:`. They run first, in the order given —
so you can compose small goals into a larger one:

```
configure:
    cmake -S . -B build

compile:
    cmake --build build

# build has no body of its own; it just runs configure, then compile
build: configure compile
```

```
va build            # runs configure, then compile
```

Dependencies run **deps-first**, each **at most once per invocation** (a shared
dependency isn't repeated), and the goal's own body runs last. The first failure
stops the sequence. The dependency graph is checked before anything runs, so a
cycle is a clear error rather than an infinite loop:

```
va: dependency cycle: build -> compile -> build
```

## Subcommands

Goals can be grouped into namespaces with `::`. A goal named `docker::build`
becomes the subcommand `va docker build`:

```
docker::build:
    docker build -t app .

docker::push:
    docker push app
```

```
va docker build     # runs docker::build
va docker push      # runs docker::push
va docker           # lists the docker subcommands
```

A namespace can also have a **default** goal — the plain goal with the same name.
Given both `build:` and `build::release:`, `va build` runs the default and
`va build release` runs the sub-goal.

### One goal per invocation

This is the main place `va` differs from `just`. In `just` you can run several
recipes at once — `just configure compile test`. In `va` an invocation always
selects **exactly one** goal; the tokens after it are arguments or a subcommand
path, never additional goals. Running things in sequence is expressed *in the
vafile* as dependencies (`build: configure compile`), not assembled on the
command line.

A consequence: a token that matches a subcommand is always treated as part of
the path, so it can never be mistaken for an argument.

## Imports

A vafile can pull goals in from other files with `import`, so shared recipes
live in one place and projects compose them. The path is **quoted** and resolved
**relative to the importing file's directory**:

```
import "ci/common.vafile"
import "docker.vafile" as docker

build: lint docker::build
    echo "everything built"
```

There are two shapes:

- **Flat** — `import "ci/common.vafile"` merges the imported goals under their
  own names. A `lint:` over there becomes `va lint` here.
- **Namespaced** — `import "docker.vafile" as docker` nests the *whole* file
  under a namespace. Its `build:` becomes `va docker build` (and `docker::build`
  when referenced as a dependency). The imported file's own internal
  dependencies are rewritten to match, so it doesn't need to know it was
  namespaced — the importer decides the layout.

Once merged, imported goals are ordinary goals: you can depend on them
(`build: lint docker::build`) and invoke them just like local ones.

An `as` namespace can also be given an **action** — a default goal — by defining
the bare name in the importing file. It runs on `va docker`, while the
subcommands still work:

```
import "docker.vafile" as docker

# `va docker` builds then pushes; `va docker build` / `va docker push` still work
docker: docker::build docker::push
```

A few rules, in keeping with va's "checked up front" stance:

- **Every name is unique.** If two files define the same final goal name it's a
  hard error, reported before anything runs. Namespacing with `as` is how you
  disambiguate.
- **Imports are transitive.** An imported file may import in turn; an import
  cycle is reported, not followed.
- **The local file doesn't win.** There's no override — a clash is always an
  error, whether between the root and an import or between two imports.
- **An `as` namespace is sealed.** Exactly one import fills it. The importing
  file may give it a default goal (the bare `docker:` above) but may not add new
  sub-goals to it or override its members, and two imports can't share one
  namespace. The contents of a namespace come from one place.

> A line is read as an import only when it's the word `import` followed by a
> quoted path (`import "x"`). That keeps a goal you legitimately *named* `import`
> (written `import:`) from being mistaken for a directive.

## Releasing

Releases are built by GitHub Actions
([`.github/workflows/release.yml`](.github/workflows/release.yml)). Each target
builds **natively on its own runner** — Linux x86_64 on Ubuntu, macOS Apple
Silicon on a macOS runner — so there's no cross-compiling, Docker, or extra
toolchain. To cut a release, bump the version in `Cargo.toml`, commit, then push
a matching tag:

```
git tag v0.1.0
git push origin v0.1.0
```

CI builds both binaries and attaches them to the GitHub Release for the tag,
which is what the [Install](#install) links point at.

---

_Status: v0 prototype._

## The name

`va` is short — most of the good short command names are already taken, and a
project runner is something you type all day. It's also French: the imperative
of *aller* ("to go"), so `va` means **"go"**. Each invocation then reads like a
little instruction — `va build` is "go build", `va test` is "go test".

## Motivation

I reach for a `justfile` in most of my projects, but I kept wanting real
**subcommands** — grouping related recipes under a namespace like
`va docker build` instead of flat names. That itch is what `va` scratches.

I mostly write Python and C++, but a small, fast, single-binary CLI is a better
fit for **Rust** — no runtime to ship and nothing to install alongside it. I
don't know Rust's exact syntax, though, so I built this by vibe-coding with
[Claude](https://www.anthropic.com/claude): I described the behavior and the
design choices I wanted, and Claude wrote and explained the Rust. Credit where
it's due — `va` was written with Claude.
