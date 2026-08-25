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
      systems = builtins.attrNames makerPackage.packages;
    in {
      packages = builtins.listToAttrs (map (system: {
        name = system;
        value = {
          maker = makerPackage.packages.${system}.default;
          maker-lgx = makerPackage.packages.${system}.lgx;
          maker-install = makerPackage.packages.${system}.install;
          maker-integration-test = makerPackage.packages.${system}.integration-test;
          taker = takerPackage.packages.${system}.default;
          taker-lgx = takerPackage.packages.${system}.lgx;
          taker-install = takerPackage.packages.${system}.install;
          taker-integration-test = takerPackage.packages.${system}.integration-test;
          default = makerPackage.packages.${system}.default;
        };
      }) systems);
      checks = builtins.listToAttrs (map (system: {
        name = system;
        value = {
          maker = makerPackage.packages.${system}.integration-test;
          taker = takerPackage.packages.${system}.integration-test;
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
