{
  description = "rawgrep";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    nixpkgs,
    crane,
    rust-overlay,
    ...
  }: let
    forAllSystems = f:
      nixpkgs.lib.genAttrs nixpkgs.lib.systems.flakeExposed (system: let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [(import rust-overlay)];
        };
        rustToolchain = pkgs.rust-bin.stable.latest.default;
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
      in
        f {
          inherit system pkgs rustToolchain craneLib;
        });

    mkRawgrep = {
      pkgs,
      craneLib,
      ...
    }: let
      src = craneLib.cleanCargoSource ./.;

      commonArgs = {
        inherit src;
        strictDeps = true;
        buildInputs = [];
      };

      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
    in
      craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          doCheck = false;
          meta = with pkgs.lib; {
            description = "Grep at the speed of raw disk";
            homepage = "https://github.com/rakivo/rawgrep";
            license = licenses.mit;
            maintainers = [];
          };
        });
  in {
    packages = forAllSystems (args: {default = mkRawgrep args;});
    devShells = forAllSystems ({
        pkgs,
        rustToolchain,
        ...
      } @ args: {
        default = pkgs.mkShell {
          inputsFrom = [(mkRawgrep args)];
          packages = with pkgs; [
            rustToolchain
            rust-analyzer
          ];
        };
      });
  };
}
