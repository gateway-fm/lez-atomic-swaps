{
  description = "LEZ atomic-swap Maker and Taker Basecamp packages";

  inputs = {
    logos-module-builder.follows = "chat_module/logos-module-builder";
    chat_module.url = "github:logos-co/logos-chat-module/v0.2.2";
    logos-delivery-module.follows = "chat_module/logos-delivery-module";
  };

  outputs = inputs@{ logos-module-builder, logos-delivery-module, ... }:
    let
      commonSource = ./common;
      injectCommon = ''
        cp ${commonSource}/local_json_rpc_client.h src/local_json_rpc_client.h
        cp ${commonSource}/local_json_rpc_client.cpp src/local_json_rpc_client.cpp
        cp ${commonSource}/logos_chat_bridge.h src/logos_chat_bridge.h
        cp ${commonSource}/logos_chat_bridge.cpp src/logos_chat_bridge.cpp
      '';
      makerPackage = logos-module-builder.lib.mkLogosQmlModule {
        src = ./maker;
        configFile = ./maker/metadata.json;
        flakeInputs = { delivery_module = logos-delivery-module; } // inputs;
        preConfigure = injectCommon;
      };
      takerPackage = logos-module-builder.lib.mkLogosQmlModule {
        src = ./taker;
        configFile = ./taker/metadata.json;
        flakeInputs = { delivery_module = logos-delivery-module; } // inputs;
        preConfigure = injectCommon;
      };
      # Qt Remote Objects creates local sockets below TMPDIR. Nix's default
      # per-build directory can exceed Linux's AF_UNIX path limit before the
      # module-specific socket name is appended, so keep the official test
      # harness unchanged while giving it a short private runtime root.
      withShortRuntimePath = package: package.overrideAttrs (previous: {
        buildCommand = ''
          export TMPDIR=/tmp/lez-ui
          export XDG_RUNTIME_DIR="$TMPDIR"
          mkdir -p "$TMPDIR"
          chmod 0700 "$TMPDIR"
          ${previous.buildCommand}
        '';
      });
      systems = builtins.attrNames makerPackage.packages;
    in {
      packages = builtins.listToAttrs (map (system: {
        name = system;
        value = {
          lez-maker-ui = makerPackage.packages.${system}.default;
          lez-maker-ui-lgx = makerPackage.packages.${system}.lgx;
          lez-maker-ui-install = makerPackage.packages.${system}.install;
          lez-maker-ui-integration-test = withShortRuntimePath makerPackage.packages.${system}.integration-test;
          lez-taker-ui = takerPackage.packages.${system}.default;
          lez-taker-ui-lgx = takerPackage.packages.${system}.lgx;
          lez-taker-ui-install = takerPackage.packages.${system}.install;
          lez-taker-ui-integration-test = withShortRuntimePath takerPackage.packages.${system}.integration-test;
          default = makerPackage.packages.${system}.default;
        };
      }) systems);
      checks = builtins.listToAttrs (map (system: {
        name = system;
        value = {
          lez-maker-ui = withShortRuntimePath makerPackage.packages.${system}.integration-test;
          lez-taker-ui = withShortRuntimePath takerPackage.packages.${system}.integration-test;
        };
      }) systems);
      apps = builtins.listToAttrs (map (system: {
        name = system;
        value = {
          maker = makerPackage.apps.${system}.default;
          taker = takerPackage.apps.${system}.default;
          default = makerPackage.apps.${system}.default;
        };
      }) systems);
    };
}
