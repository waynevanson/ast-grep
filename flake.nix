{
  description = "ast-grep: structural search and rewrite code using AST patterns";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, fenix, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      eachSystem = nixpkgs.lib.genAttrs systems;
      perSystem =
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          inherit (nixpkgs.lib) importTOML;
          version = (importTOML ./Cargo.toml).workspace.package.version;

          # Derive the toolchain from rust-toolchain.toml so Nix builds use the
          # same channel and components as the rest of the repository.
          rustToolchain = fenix.packages.${system}.fromToolchainFile {
            file = ./rust-toolchain.toml;
            sha256 = "sha256-MFHoWVcPZKjFrLnQnodCXbUsfZcofGfo9tV9fTTvklg=";
          };
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };

          # `cleanSource` filters out `.so` files by default, but the CLI
          # integration tests need the prebuilt dynamic language fixtures.
          testSrc = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter =
              path: type:
              pkgs.lib.cleanSourceFilter path type
              || pkgs.lib.hasPrefix "${toString ./fixtures}/" (toString path);
          };

          # Wrap a cargo command in the standard Rust build setup so that the
          # workspace dependencies are vendored and available offline in the
          # Nix sandbox.
          mkCargoCheck =
            name: command:
            rustPlatform.buildRustPackage {
              pname = "ast-grep-${name}";
              inherit version;

              src = testSrc;

              cargoLock.lockFile = ./Cargo.lock;

              doCheck = false;

              buildPhase = ''
                runHook preBuild
                ${command}
                runHook postBuild
              '';

              installPhase = ''
                runHook preInstall
                mkdir -p $out
                runHook postInstall
              '';

              meta = {
                description = "ast-grep ${name}";
                homepage = "https://ast-grep.github.io/";
                license = pkgs.lib.licenses.mit;
              };
            };
        in
        rec {
          ast-grep = rustPlatform.buildRustPackage {
            pname = "ast-grep";
            inherit version;

            src = pkgs.lib.cleanSource ./.;

            cargoLock.lockFile = ./Cargo.lock;

            # Only build the CLI; the workspace also contains bindings crates
            # (napi, pyo3, wasm) that are not needed here.
            cargoBuildFlags = [
              "--package"
              "ast-grep"
            ];
            # The full workspace test suite runs in the `checks` output;
            # keep the package build lean.
            doCheck = false;

            nativeBuildInputs = [ pkgs.installShellFiles ];

            postInstall = ''
              installShellCompletion --cmd ast-grep \
                --bash <($out/bin/ast-grep completions bash) \
                --zsh <($out/bin/ast-grep completions zsh) \
                --fish <($out/bin/ast-grep completions fish)
            '';

            meta = {
              description = "Structural search and rewrite code using AST patterns";
              homepage = "https://ast-grep.github.io/";
              license = pkgs.lib.licenses.mit;
              mainProgram = "ast-grep";
            };
          };

          checks = {
            # `cargo check` the whole workspace, treating warnings as errors.
            cargo-check = mkCargoCheck "cargo-check" ''
              RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --locked --offline
            '';
            # Run the full workspace test suite.
            cargo-test = mkCargoCheck "cargo-test" ''
              cargo test --workspace --all-targets --locked --offline
            '';
          };

          default = ast-grep;

          devShell = pkgs.mkShell {
            inputsFrom = [ ast-grep ];
            # Use rust-analyzer from the fenix toolchain: it matches the
            # rustc/cargo/rust-src components configured in
            # rust-toolchain.toml, so it can find the standard library sources.
            packages = [ rustToolchain ];

            shellHook = ''
              export RUST_SRC_PATH="${rustToolchain}/lib/rustlib/src/rust/library"
            '';
          };

          formatter = pkgs.nixfmt;
        };
    in
    {
      packages = eachSystem (system: {
        default = (perSystem system).default;
        ast-grep = (perSystem system).ast-grep;
      });

      devShells = eachSystem (system: {
        default = (perSystem system).devShell;
      });

      checks = eachSystem (system: (perSystem system).checks);

      formatter = eachSystem (system: (perSystem system).formatter);

      overlays.default = final: prev: {
        ast-grep = (perSystem prev.stdenv.hostPlatform.system).ast-grep;
      };
    };
}
