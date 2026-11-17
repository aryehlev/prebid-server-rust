compile_error!(
    "rust/patches/ring stands in for ring, which nothing used to build: rustls-webpki's `alloc` \
     feature only names ring as `ring?/alloc`, so Cargo.lock listed it with windows-sys 0.52 \
     (rust/PORTING.md section 2). A crate now enables ring for real, so this stub must be \
     replaced: remove `ring` from [patch.crates-io] in rust/Cargo.toml and resolve the \
     windows-sys duplicate that the real ring brings."
);
