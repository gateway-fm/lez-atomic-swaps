{
  description = "LEZ atomic-swap Maker Basecamp package";

  # This is the per-package flake a Logos module catalog builds
  # (logos-modules-release-action runs `nix build .#lgx-portable` inside this
  # directory). The aggregate flake one level up (../flake.nix) builds both
  # role packages together for the repository's own tests; both pin the same
  # Chat release, so keep the two flake.lock files identical.
  inputs = {
    logos-module-builder.follows = "chat_module/logos-module-builder";
    chat_module.url = "github:logos-co/logos-chat-module/v0.2.2";
    logos-delivery-module.follows = "chat_module/logos-delivery-module";
  };

  outputs = inputs@{ logos-module-builder, logos-delivery-module, ... }:
    let
      # The shared local-RPC client, Chat bridge and Node market sources live
      # next to both packages. A flake evaluated from inside a git tree sees
      # the whole tree, so the sibling directory resolves; a bare copy of this
      # directory alone does not build.
      commonSource = ../common;
      injectCommon = ''
        cp ${commonSource}/local_json_rpc_client.h src/local_json_rpc_client.h
        cp ${commonSource}/local_json_rpc_client.cpp src/local_json_rpc_client.cpp
        cp ${commonSource}/logos_chat_bridge.h src/logos_chat_bridge.h
        cp ${commonSource}/logos_chat_bridge.cpp src/logos_chat_bridge.cpp
        cp ${commonSource}/node_market.h src/node_market.h
        cp ${commonSource}/node_market.cpp src/node_market.cpp
      '';
      package = logos-module-builder.lib.mkLogosQmlModule {
        src = ./.;
        configFile = ./metadata.json;
        flakeInputs = { delivery_module = logos-delivery-module; } // inputs;
        preConfigure = injectCommon;
      };
    in {
      # Everything the module builder provides: default, lgx, lgx-portable,
      # install, integration-test, ...
      inherit (package) packages apps;
    };
}
