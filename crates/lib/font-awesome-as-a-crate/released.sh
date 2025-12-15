#!/bin/sh
# Running this script produces a crate with no build script.
# It should make compilation a smidge faster, by avoiding a `.crate` file
# with tons and tons of stuff in it.
set -ex

rm -rf released
mkdir released
cp -r Cargo.toml Cargo.lock README.md src font*/LICENSE.txt released
rustc build.rs
OUT_DIR=released/src ./build
rm build
cat > released/released.sh <<EOF
#!/bin/sh
echo "This script only makes sense on a source code checkout from version control."
echo "It does nothing on a crates.io release."
EOF

