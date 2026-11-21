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
            # Tests depend on fixtures and external tooling; keep the build lean.
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

      formatter = eachSystem (system: (perSystem system).formatter);

      overlays.default = final: prev: {
        ast-grep = (perSystem prev.stdenv.hostPlatform.system).ast-grep;
      };
    };
}
