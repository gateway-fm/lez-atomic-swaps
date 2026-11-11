{
  description = "LEZ Taker Node payload (universal core module, logos-module-builder)";

  # The plugin only: the Node's executables are added to the package's
  # variant by deploy/scripts/package-role-node.sh (they are built by Cargo,
  # outside Nix). Same pins as modules/lez-swap-node (the lock file is a copy).
  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";
  };

  outputs =
    inputs@{ logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
