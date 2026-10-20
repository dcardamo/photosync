{ config, lib, pkgs, ... }:
let
  cfg = config.services.photosync-status;
  settings = cfg.settings;
  configFile = (pkgs.formats.toml {}).generate "photosync-status.toml" settings;
in {
  options.services.photosync-status = {
    enable = lib.mkEnableOption "the read-only photosync status server";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../nix/package.nix {};
      description = "photosync-status package to run.";
    };

    settings = lib.mkOption {
      type = lib.types.attrs;
      default = {};
      description = ''
        contents of the status server TOML config, for example:
        {
          bind = "127.0.0.1:8787";
          root = "/srv/photos";
          ignore_file = ".stignore";
          backup_marker = "/var/lib/restic/last-success";
          sizes_cache_seconds = 60;
        }
      '';
    };

    tokenFile = lib.mkOption {
      type = lib.types.path;
      description = "File containing the bearer token clients must present.";
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "photosync-status";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "photosync-status";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open the configured bind port in the firewall.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = settings ? root;
        message = "services.photosync-status.settings.root is required";
      }
      {
        assertion = settings ? bind;
        message = "services.photosync-status.settings.bind is required";
      }
    ];

    users.users.${cfg.user} = lib.mkIf (cfg.user == "photosync-status") {
      isSystemUser = true;
      group = cfg.group;
    };
    users.groups.${cfg.group} = lib.mkIf (cfg.group == "photosync-status") {};

    systemd.services.photosync-status = {
      description = "photosync read-only status server";
      wantedBy = ["multi-user.target"];
      after = ["network.target"];
      serviceConfig = {
        ExecStart = "${cfg.package}/bin/photosync-status --config ${configFile} --token-file %d/photosync-token";
        LoadCredential = ["photosync-token:${cfg.tokenFile}"];
        User = cfg.user;
        Group = cfg.group;
        NoNewPrivileges = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictAddressFamilies = ["AF_INET" "AF_INET6" "AF_UNIX"];
        RestrictNamespaces = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
        ReadOnlyPaths = [settings.root];
      };
    };

    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [
      (lib.toInt (lib.last (lib.splitString ":" settings.bind)))
    ];
  };
}
