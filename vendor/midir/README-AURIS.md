# Auris Studio patch

This directory contains `midir` 0.11.0 from crates.io.

The only source-package change is that the macOS and iOS `coremidi` dependency is
kept on the compatible 0.8 series. `midir` 0.11.0 normally requests `coremidi`
0.9, which requires `core-foundation` 0.10.1 or newer. GPUI 0.2.2 pins
`core-foundation` exactly to 0.10.0, so Cargo cannot resolve the two published
packages together.

The fork can be removed once GPUI no longer pins `core-foundation` 0.10.0, or
once an upstream `midir` release supports both dependency sets.
