{
  description = "codex-switch Rust CLI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { nixpkgs, ... }:
    let
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;

      mkPackage =
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = cargoToml.package.name;
          version = cargoToml.package.version;

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs = [
            pkgs.cmake
          ];

          buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin (
            with pkgs.darwin.apple_sdk.frameworks;
            [
              CoreFoundation
              Security
              SystemConfiguration
            ]
          );

          meta = {
            description = "Switch local Codex account profiles";
            license = pkgs.lib.licenses.mit;
            mainProgram = "codex-switch";
          };
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          package = mkPackage system;
        in
        {
          codex-switch = package;
          default = package;
        }
      );
    };
}
