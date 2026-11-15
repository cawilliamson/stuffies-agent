{
  description = "sa — daemon-first coding agent";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-linux"
        "x86_64-linux"
      ];
      each = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in
    {
      devShells = each (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            ast-grep
            cargo
            cargo-deny
            cargo-machete
            cargo-mutants
            clippy
            marksman
            python3
            rust-code-analysis
            rustc
            rustfmt
          ];
        };
      });

      checks = each (pkgs: {
        deps = pkgs.runCommand "sa-check-deps" { nativeBuildInputs = [ pkgs.cargo ]; } ''
          cp -r ${self} src
          chmod -R u+w src
          cd src
          ./ci/check-deps.sh
          touch $out
        '';
      });
    };
}
