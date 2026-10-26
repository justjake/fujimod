//! Links the static Unicorn archive that the build script compiles from
//! `../vendor/unicorn`. The C bindings live in `fujimod-emulator-cpu-unicorn`.
//!
//! The archive build has no features, so every feature set, test and binary of
//! the CPU crate shares one Unicorn compilation per profile and target.
#![no_std]
