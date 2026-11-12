{
  description = "4k free video dowloader (vidgrab) - Open-source Rust 4K/8K video & audio downloader";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          cargoLock.lockFile = ../../Cargo.lock;
          runtimeTools = with pkgs; [ ffmpeg yt-dlp wl-clipboard xclip ];
        in {
          vidgrab-cli = pkgs.rustPlatform.buildRustPackage {
            pname = "vidgrab-cli";
            version = "0.3.0";
            src = ../..;
            inherit cargoLock;
            cargoBuildFlags = [ "-p" "vidgrab-cli" ];
            nativeBuildInputs = [ pkgs.pkg-config pkgs.makeWrapper ];
            buildInputs = [ pkgs.openssl pkgs.sqlite ];
            postInstall = ''
              wrapProgram $out/bin/vidgrab-cli \
                --prefix PATH : ${pkgs.lib.makeBinPath runtimeTools}
            '';
          };

          vidgrab = pkgs.rustPlatform.buildRustPackage {
            pname = "vidgrab";
            version = "0.3.0";
            src = ../..;
            inherit cargoLock;
            cargoBuildFlags = [ "-p" "vidgrab-tauri" "-p" "vidgrab-cli" ];
            nativeBuildInputs = [ pkgs.pkg-config pkgs.makeWrapper pkgs.wrapGAppsHook3 ];
            buildInputs = with pkgs; [
              webkitgtk_4_1
              gtk3
              openssl
              sqlite
              librsvg
              libayatana-appindicator
            ];
            postInstall = ''
              install -Dm644 packaging/vidgrab.desktop $out/share/applications/vidgrab.desktop
              install -Dm644 packaging/metainfo/io.github.vidgrab.VidGrab.metainfo.xml $out/share/metainfo/io.github.vidgrab.VidGrab.metainfo.xml
              install -Dm644 src-tauri/icons/128x128.png $out/share/icons/hicolor/128x128/apps/vidgrab.png
              wrapProgram $out/bin/vidgrab \
                --prefix PATH : ${pkgs.lib.makeBinPath runtimeTools}
            '';
          };

          default = self.packages.${system}.vidgrab;
        });
    };
}
