{
  description = "ast-grep: structural search and rewrite code using AST patterns";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { nixpkgs, ... }:
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
        in
        rec {
          ast-grep = pkgs.rustPlatform.buildRustPackage {
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
            packages = with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              rust-analyzer
            ];
            # Keep the toolchain aligned with the repository's MSRV.
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
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
