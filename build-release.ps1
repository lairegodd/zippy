# Dependency panic paths would otherwise embed the builder's home folder (and user name) in zippy.exe.
$ErrorActionPreference = 'Stop'
$env:CARGO_ENCODED_RUSTFLAGS = "--remap-path-prefix=$env:USERPROFILE=~"
cargo build --release
exit $LASTEXITCODE
