{
  lib,
  rustPlatform,
  fetchFromGitHub,
  cmake,
  pkg-config,
  libxml2,
}:

rustPlatform.buildRustPackage (finalAttrs: {
  pname = "camel";
  version = "0.28.0";

  src = fetchFromGitHub {
    owner = "kennycallado";
    repo = "rust-camel";
    tag = "v${finalAttrs.version}";
    hash = "sha256-f9lBYu3RI5lKp1t3BZlacWiw9xQl6mFudJ1Cuu4v1DY=";
  };

  cargoLock.lockFile = "${finalAttrs.src}/Cargo.lock";

  nativeBuildInputs = [
    cmake
    pkg-config
  ];
  buildInputs = [ libxml2 ];

  cargoBuildFlags = [ "-p camel-cli" ];
  cargoTestFlags = [
    "-p"
    "camel-cli"
    "--lib"
  ];

  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck
    $out/bin/camel --help >/dev/null
    runHook postInstallCheck
  '';

  meta = {
    description = "Rust integration framework inspired by Apache Camel";
    homepage = "https://github.com/kennycallado/rust-camel";
    changelog = "https://github.com/kennycallado/rust-camel/releases/tag/v${finalAttrs.version}";
    license = lib.licenses.asl20;
    mainProgram = "camel";
    maintainers = [ ];
    platforms = lib.platforms.unix;
  };
})
