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
        cp ${commonSource}/node_market.h src/node_market.h
        cp ${commonSource}/node_market.cpp src/node_market.cpp
      '';
      # The QML view directory is copied from the package source as-is, so
      # the shared UI kit (common/qml) is merged into each role's src/qml in
      # a derived source rather than injected at configure time.
      nixpkgsFor = system: logos-module-builder.inputs.nixpkgs.legacyPackages.${system};
      withKit = system: role: (nixpkgsFor system).runCommand "lez-${role}-ui-source" {} ''
        cp -r ${./. + "/${role}"} $out
        chmod -R u+w $out
        cp ${commonSource}/qml/*.qml $out/src/qml/
      '';
      packageFor = system: role: logos-module-builder.lib.mkLogosQmlModule {
        src = withKit system role;
        configFile = ./. + "/${role}/metadata.json";
        flakeInputs = { delivery_module = logos-delivery-module; } // inputs;
        preConfigure = injectCommon;
      };
      probe = logos-module-builder.lib.mkLogosQmlModule {
        src = ./maker;
        configFile = ./maker/metadata.json;
        flakeInputs = { delivery_module = logos-delivery-module; } // inputs;
        preConfigure = injectCommon;
      };
      makerFor = system: packageFor system "maker";
      takerFor = system: packageFor system "taker";
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
      systems = builtins.attrNames probe.packages;
    in {
      packages = builtins.listToAttrs (map (system: let maker = makerFor system; taker = takerFor system; in {
        name = system;
        value = {
          lez-maker-ui = maker.packages.${system}.default;
          lez-maker-ui-lgx = maker.packages.${system}.lgx;
          lez-maker-ui-install = maker.packages.${system}.install;
          lez-maker-ui-integration-test = withShortRuntimePath maker.packages.${system}.integration-test;
          lez-taker-ui = taker.packages.${system}.default;
          lez-taker-ui-lgx = taker.packages.${system}.lgx;
          lez-taker-ui-install = taker.packages.${system}.install;
          lez-taker-ui-integration-test = withShortRuntimePath taker.packages.${system}.integration-test;
          default = maker.packages.${system}.default;
        };
      }) systems);
      checks = builtins.listToAttrs (map (system: let maker = makerFor system; taker = takerFor system; in {
        name = system;
        value = {
          lez-maker-ui = withShortRuntimePath maker.packages.${system}.integration-test;
          lez-taker-ui = withShortRuntimePath taker.packages.${system}.integration-test;
        };
      }) systems);
      apps = builtins.listToAttrs (map (system: let maker = makerFor system; taker = takerFor system; in {
        name = system;
        value = {
          maker = maker.apps.${system}.default;
          taker = taker.apps.${system}.default;
          default = maker.apps.${system}.default;
        };
      }) systems);
    };
}
