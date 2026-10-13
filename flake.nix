{
  description = "ferrocraft: a Minecraft Java Edition server written in Rust";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        packages = [
          pkgs.cargo
          pkgs.rustc
          pkgs.clippy
          pkgs.rustfmt
          pkgs.rust-analyzer
          # Only for scripts/gen-reports.sh, which runs Mojang's data generator
          # to read packet IDs and registries off the vanilla server jar.
          pkgs.jdk25_headless
          pkgs.jq
        ];
        RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
      };

      formatter.${system} = pkgs.nixfmt;
    };
}
