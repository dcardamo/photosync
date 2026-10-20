{ lib, rustPlatform }:
rustPlatform.buildRustPackage rec {
  pname = "photosync-status";
  version = "0.1.0";

  src = ../.;

  cargoLock.lockFile = ../Cargo.lock;

  cargoBuildFlags = ["-p" "photosync-status"];
  cargoTestFlags = ["-p" "photosync-status"];

  installPhase = ''
    runHook preInstall
    binary=$(find target -path '*/release/photosync-status' -type f | head -1)
    test -n "$binary"
    install -Dm755 "$binary" "$out/bin/photosync-status"
    runHook postInstall
  '';

  meta = {
    description = "Read-only status server for the photosync release gate";
    license = with lib.licenses; [asl20 mit];
    mainProgram = "photosync-status";
  };
}
