{
  description = "LEZ atomic-swap Maker and Taker Basecamp packages";

  inputs.logos-module-builder.url = "github:logos-co/logos-module-builder/0.2.0";

  outputs = inputs@{ logos-module-builder, ... }:
    let
      commonSource = ./common;
      injectCommon = ''
        cp ${commonSource}/local_json_rpc_client.h src/local_json_rpc_client.h
        cp ${commonSource}/local_json_rpc_client.cpp src/local_json_rpc_client.cpp
      '';
      makerPackage = logos-module-builder.lib.mkLogosQmlModule {
        src = ./maker;
        configFile = ./maker/metadata.json;
        flakeInputs = inputs;
        preConfigure = injectCommon;
      };
      takerPackage = logos-module-builder.lib.mkLogosQmlModule {
        src = ./taker;
        configFile = ./taker/metadata.json;
        flakeInputs = inputs;
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
