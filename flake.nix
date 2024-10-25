# forked from https://github.com/tosc-rs/mnemos/blob/main/flake.nix
{
    description = "Flake providing a development shell for kiwi";

    inputs = {
        nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
        flake-utils.url = "github:numtide/flake-utils";
        rust-overlay = {
            url = "github:oxalica/rust-overlay";
            inputs = {
                nixpkgs.follows = "nixpkgs";
            };
        };
    };

    outputs = { nixpkgs, flake-utils, rust-overlay, ... }:
        flake-utils.lib.eachDefaultSystem (system:
            let
                overlays = [ (import rust-overlay) ];
                pkgs = import nixpkgs { inherit system overlays; };
                # use the Rust toolchain specified in the project's rust-toolchain.toml
                rustToolchain = pkgs.pkgsBuildHost.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
            in
            {
                devShell = with pkgs; mkShell rec {
                    name = "kiwi-dev";
                    nativeBuildInputs = [
                        # compilers
                        rustToolchain
                        clang
                        cmake

                        # devtools
                        wabt
                        binaryen
                        trunk
                    ];
                    buildInputs = [
                        # misc. libraries
                        openssl
                        pkg-config

                        # GUI libs
                        libxkbcommon
                        libGL
                        fontconfig

                        # wayland libraries
                        wayland

                        # x11 libraries
                        xorg.libXcursor
                        xorg.libXrandr
                        xorg.libXi
                        xorg.libX11
                    ];

                    LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
                    LD_LIBRARY_PATH = "${lib.makeLibraryPath buildInputs}";
                };
            }
        );
}