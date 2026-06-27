# vafile syntax (VS Code)

Syntax highlighting for va's `vafile` command-runner format.

Highlights:

- comments (`# ...` at column 0)
- recipe names and `::` namespace separators
- parameters and the optional `?` marker
- the `:` separator and dependency references after it
- `{{name}}` interpolation and `$name` / `${name}` in recipe bodies

Applies to files named `vafile` (and anything matching `*vafile`, e.g.
`example-vafile`) or with a `.vafile` extension.

## Try it without installing

Open this folder in VS Code and press **F5** ("Run Extension"). In the
Extension Development Host that opens, open a `vafile` — it will be highlighted.

## Install locally

Symlink (or copy) this folder into your VS Code extensions directory, then
reload the window:

```
ln -s "$PWD" ~/.vscode/extensions/vafile-0.0.1
```

## Package as a .vsix

```
npm install -g @vscode/vsce
vsce package
```

This is grammar-only — no runtime code, so there is nothing to compile.
