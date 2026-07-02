# Backpack Infiltrator

A desktop app for the Brassworks SMP mods that lets you search through every
container, backpack, and inventory on the server. It reads the dumps created by
the Brassworks Core mod and shows them all in one window - with search, item
tooltips, nested container contents, and real 3D player heads.

![Backpack Infiltrator](assets/img.png)

## What it reads

- `sophisticatedbackpacks.dat` - your backpacks (from `world/data/` in your save)
- `*_container_dump.json` - world containers like chests, barrels, and shulkers
- `*_player_dump.json` - player inventories and ender chests

Just drag any of these onto the window, or use the "Load files" button. You can
load several at once - loading the backpacks `.dat` alongside a dump also lets
you see the contents of backpacks sitting inside other inventories.

## Running it

```bash
cargo run --release
```

To just build it without running:

```bash
cargo build
```

`cargo build` puts the program at `target/debug/infiltrator`, and
`cargo run --release` builds a faster version at `target/release/infiltrator`.
Keep the `assets/` folder next to the program.

## Terminal helpers

If you'd rather not open the window, these run straight from the terminal:

```bash
infiltrator --parse <file>                       # print a quick summary of a file
infiltrator --png <file> [out.png]               # save the fullest entry as an image
infiltrator --head <skin.png> <out.png> [size]   # render a 3D head from a skin
```
