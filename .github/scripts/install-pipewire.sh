#!/usr/bin/env bash
# Build PipeWire 1.2.7 from source for pipewire-rs 0.10 headers.
# Ubuntu 22.04 apt only ships PipeWire 0.3.48 headers which are incompatible.
set -euo pipefail

PW_VERSION="${PW_VERSION:-1.2.7}"

echo "Building PipeWire ${PW_VERSION} from source..."

curl -sSL "https://gitlab.freedesktop.org/pipewire/pipewire/-/archive/${PW_VERSION}/pipewire-${PW_VERSION}.tar.gz" | tar xz
cd "pipewire-${PW_VERSION}"

meson setup builddir --prefix=/usr \
  -Dspa-plugins=disabled -Dpipewire-v4l2=disabled -Dpipewire-jack=disabled \
  -Dpipewire-alsa=disabled -Dlibjack-path="" -Dalsa=disabled \
  -Dsession-managers=[] -Dtests=disabled -Dexamples=disabled -Dman=disabled \
  -Ddocs=disabled -Dgstreamer=disabled -Dbluez5=disabled -Davahi=disabled \
  -Draop=disabled -Dudevrulesdir=""

ninja -C builddir
sudo ninja -C builddir install
sudo ldconfig

cd ..
rm -rf "pipewire-${PW_VERSION}"

echo "PipeWire ${PW_VERSION} installed successfully."
