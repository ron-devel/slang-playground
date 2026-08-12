#!/bin/bash

if [ ! -d emsdk ]
then
    git clone https://github.com/emscripten-core/emsdk.git emsdk
fi

pushd emsdk
	sed -i 's/\r$//' emsdk emsdk_env.sh
	/bin/sh ./emsdk install latest
	/bin/sh ./emsdk activate latest
	source ./emsdk_env.sh
popd

if [ ! -d spirv-tools ]
then
	git clone https://github.com/KhronosGroup/SPIRV-Tools.git spirv-tools
fi

pushd spirv-tools
git checkout vulkan-sdk-1.3.290.0

python3 utils/git-sync-deps

# add an additional option to emcc command
sed -i 's/\r$//' source/wasm/build.sh
# -sDEFAULT_TO_CXX is required since Emscripten 6.0.6, which disabled it by
# default; without it, plain `emcc` no longer links the C++ runtime that
# this file's embind (`--bind`) usage needs.
sed -i 's/-s MODULARIZE \\/-s MODULARIZE -s EXPORT_ES6 -sDEFAULT_TO_CXX \\/' source/wasm/build.sh

bash -x source/wasm/build.sh

cp out/web/spirv-tools.wasm ../
cp out/web/spirv-tools.js ../
cp out/web/spirv-tools.d.ts ../

popd
