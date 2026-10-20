# Example only. Wire this into your own host configuration.
{ config, pkgs, ... }:
{
  imports = [ ../nix/module.nix ];
  services.photosync-status = {
    enable = true;
    tokenFile = "/run/secrets/photosync-status-token";
    openFirewall = false;
    settings = {
      bind = "203.0.113.10:8787";
      root = "/srv/photos";
      ignore_file = ".stignore";
      backup_marker = "/var/lib/photosync/last-backup";
      sizes_cache_seconds = 60;
      tls_cert = "/var/lib/photosync/hub.example.ts.net.crt";
      tls_key = "/var/lib/photosync/hub.example.ts.net.key";
    };
  };
}
