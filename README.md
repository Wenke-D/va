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

---

_Status: v0 prototype. Single-file vafiles only; cross-file imports are not yet
supported._

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
