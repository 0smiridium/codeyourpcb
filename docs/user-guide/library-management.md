# Library Management Guide

A KiCad footprint library is a folder called `<name>.pretty` holding one
`.kicad_mod` file per footprint. `cypcb library import` reads those folders
into an index, `cypcb library search` finds a footprint in it, and a design
names that footprint as `kicad::<library>:<name>`.

## Use a KiCad footprint by name

Put the `.pretty` folders you want under a folder in your project - here
`libraries/`, holding `Test_Library.pretty` - and run, from the project
directory:

```sh
cypcb library import libraries   # prints: Test_Library: 3 footprint(s)
cypcb library search 0603        # prints: kicad::Test_Library:R_0603_1608Metric
cypcb library list               # prints: Test_Library (kicad)  3 footprint(s)
cypcb check board.cypcb          # exit 1, prints: Unconnected pin: R1.1
```

`board.cypcb` names the footprint the way `search` printed it:

```cypcb
version 1

board test {
    size 30mm x 30mm
    layers 2
}

component R1 resistor "kicad::Test_Library:R_0603_1608Metric" {
    value "330"
    at 15mm, 15mm
}
```

`check` exits 1 because the resistor's two pads are wired to nothing. The pads
are the ones the `.kicad_mod` file draws. A footprint the index does not hold
is refused as `unknown footprint`, at the line that named it.

The KiCad installation's own libraries work the same way: point `import` at
the directory holding the `.pretty` folders, for example
`/usr/share/kicad/footprints` on Linux.

## Where the index lives

The index is one SQLite file, `cypcb-library.db`.

- `cypcb library import`, `search` and `list` use `cypcb-library.db` in the
  directory they are run in. `--db <path>` names another file.
- A design reads `cypcb-library.db` in its own directory. If there is none,
  it reads the one in the nearest directory above it. A design reads only
  that one file.

Commit the `.pretty` folders with the project, not the index. `import`
rebuilds the index from them. Importing a library again makes the index hold
what its folder holds: a footprint whose file is gone leaves the index, and
`import` prints how many left.

An index written before the library was part of the name is moved to the new
form the first time it is opened, with every footprint kept.

## How a footprint name is resolved

`check`, `export`, `route`, `score`, `parse`, `to-kicad`, `watch` and the
language server all resolve a name the same way. The first source that has it
wins:

1. a `footprint` block the design defines under that name;
2. a built-in footprint (`0402`, `0603`, `SOT-23-5`, `DIP-8` and the rest);
3. the index, for a name written `source::library:name`.

A built-in name never holds `::`, so a footprint from the index never replaces
a built-in.

The library is part of the name, as in KiCad, so two libraries can each hold
a `SOT-23-5`. A name without the library, such as `kicad::R_0603_1608Metric`,
still resolves when one library alone holds it. When two or more do, the
design is refused with every one of them written in full, and you pick one:

```text
footprint 'kicad::SOT-23-5' is in more than one library: kicad::A:SOT-23-5, kicad::B:SOT-23-5
```

A library name cannot hold `:`, because the name is split at the first one.
KiCad refuses `:` in a library nickname too.

## In the browser

The viewer does not read `cypcb-library.db`. A page has no access to your
files. A design that names `kicad::<library>:<name>` opens in the viewer with that part
reported as `unknown footprint`. The viewer resolves only the design's own
`footprint` blocks, the built-ins, and the parts it fetches itself.

To take a design with library footprints to the browser, define those parts in
the design as `footprint` blocks.

## Commands

| Command | What it does |
|---|---|
| `cypcb library import <directory>` | Reads every `<name>.pretty` folder under `<directory>` into the index and prints how many footprints each one brought |
| `cypcb library search <query> [--limit N]` | Finds footprints by name, description, package or manufacturer, and prints each name the way a design writes it |
| `cypcb library list` | Lists the imported libraries and their footprint counts |

Each command takes `--db <path>`.

## Verification

```sh
cargo test -p cypcb-cli --test the_library_guide_runs
```

The test reads the commands and the design from this page, runs them in an
empty directory against `Test_Library.pretty` from the repository's fixtures,
and fails when a command does not print or exit the way this page says.

Last verified: 2026-09-27
