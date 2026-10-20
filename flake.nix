{
  description = "photosync: choose which Syncthing photo projects a travel device keeps";

  inputs = {
    # Pinned so the Android SDK, NDK and emulator image come from one known
    # nixpkgs revision. Keep in step with the release workflow.
    nixpkgs.url = "github:nixos/nixpkgs/c59305bab2065cfecc4944690d9eedbb56f3a9fa";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
  }: let
    system = "x86_64-linux";
    pkgs = import nixpkgs {
      inherit system;
      overlays = [rust-overlay.overlays.default];
      config = {
        allowUnfree = true;
        android_sdk.accept_license = true;
      };
    };

    rust = pkgs.rust-bin.stable.latest.default.override {
      extensions = ["rust-src" "clippy" "rustfmt"];
      targets = [
        "x86_64-unknown-linux-gnu"
        "aarch64-linux-android"
        "x86_64-linux-android"
      ];
    };

    ndkVersion = "27.2.12479018";
    android = pkgs.androidenv.composeAndroidPackages {
      platformVersions = ["35" "36"];
      buildToolsVersions = ["35.0.0" "36.0.0"];
      includeNDK = true;
      ndkVersions = [ndkVersion];
      includeEmulator = true;
      includeSystemImages = true;
      systemImageTypes = ["google_apis_playstore"];
      abiVersions = ["x86_64"];
      cmakeVersions = ["3.22.1"];
    };
    sdk = "${android.androidsdk}/libexec/android-sdk";

    # Tauri's desktop runtime links WebKitGTK; needed to build and test the
    # app crate on the host.
    linuxDeps = with pkgs; [
      webkitgtk_4_1
      gtk3
      libsoup_3
      glib
      glib-networking
      cairo
      pango
      gdk-pixbuf
      atk
      librsvg
      openssl
      dbus
      libayatana-appindicator
    ];
  in {
    packages.${system} = {
      photosync-status = pkgs.callPackage ./nix/package.nix {};
      default = self.packages.${system}.photosync-status;
    };

    nixosModules.default = import ./nix/module.nix;

    devShells.${system}.default = pkgs.mkShell {
      nativeBuildInputs = with pkgs; [
        rust
        pkg-config
        cargo-tauri
        nodejs_22
        jdk17
        android.androidsdk
        sqlite
        imagemagick
        jq
        gnumake
        gitleaks
        syncthing
        docker-compose
      ];
      buildInputs = linuxDeps;

      ANDROID_HOME = sdk;
      ANDROID_SDK_ROOT = sdk;
      NDK_HOME = "${sdk}/ndk/${ndkVersion}";
      JAVA_HOME = pkgs.jdk17.home;
      # Gradle downloads a dynamically linked aapt2; use the SDK's patched one.
      GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${sdk}/build-tools/35.0.0/aapt2";

      shellHook = ''
        export GIO_MODULE_DIR=${pkgs.glib-networking}/lib/gio/modules
        export XDG_DATA_DIRS=${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}:$XDG_DATA_DIRS
        export LD_LIBRARY_PATH=${pkgs.lib.makeLibraryPath linuxDeps}:$LD_LIBRARY_PATH
        export SYNCTHING_BIN=${pkgs.syncthing}/bin/syncthing
        # Worktrees keep their own Android state, independent of the entry directory.
        photosync_root="$(git rev-parse --show-toplevel 2>/dev/null)" || photosync_root="$PWD"
        while [ ! -f "$photosync_root/flake.nix" ] || [ ! -d "$photosync_root/app/src-tauri" ]; do
          if [ "$photosync_root" = / ]; then
            echo "Enter the photosync dev shell from within its checkout or source archive." >&2
            exit 1
          fi
          photosync_root="$(dirname "$photosync_root")"
        done
        export ANDROID_AVD_HOME="$photosync_root/.android-avd"
        export ANDROID_USER_HOME="$photosync_root/.android-user"
        export ANDROID_SERIAL=''${ANDROID_SERIAL:-emulator-5560}
        if [ "$(git -C "$photosync_root" rev-parse --show-toplevel 2>/dev/null)" = "$photosync_root" ]; then
          [ "$(git -C "$photosync_root" config --get core.hooksPath)" = .githooks ] || git -C "$photosync_root" config core.hooksPath .githooks
        fi
        unset photosync_root
      '';
    };
  };
}
