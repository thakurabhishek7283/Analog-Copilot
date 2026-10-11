#!/usr/bin/env bash
# P5.0 spike: ngspice 47 WASM with XSPICE and statically linked code models (analog, digital,
# spice2poly). Runs in emscripten/emsdk:6.0.10 with /ngspice = third_party/ngspice (read only)
# and /spike = this directory. Output: /spike/dist/ngspice.{mjs,wasm}, logs in /spike/logs.
set -euo pipefail
work=/tmp/xs; rm -rf $work; mkdir -p $work /spike/logs /spike/dist
tar -xzf /ngspice/.cache/ngspice-47.tar.gz -C $work
cd $work/ngspice-47
for p in /ngspice/patches/*.patch; do patch -p1 -s < "$p"; done
# load_opus: static code models first
perl -0pi -e 's|(int load_opus\(const char \*name\)\n\{.*?\n)(    lib = dlopen\(name, RTLD_NOW\);)|$1    {\n        extern int ng_static_cm(const char *, int *, SPICEdev ***, int *, Evt_Udn_Info_t ***);\n        int sn, su; SPICEdev **sd; Evt_Udn_Info_t **sudn;\n        if (ng_static_cm(name, &sn, &sd, &su, &sudn) == 0) { add_device(sn, sd, 1); add_udn(su, sudn); return 0; }\n    }\n$2|s' src/spicelib/devices/dev.c
grep -q ng_static_cm src/spicelib/devices/dev.c || { echo "dev.c patch failed"; exit 1; }

log=/spike/logs/build.log
emconfigure ./configure --with-ngshared --enable-xspice --disable-osdi --disable-openmp --disable-klu \
  --with-readline=no --with-fftw3=no --without-x --disable-dependency-tracking \
  --enable-static --disable-shared --without-pic CFLAGS="-O2 -Dlow_latency" CXXFLAGS="-O2" > /spike/logs/configure.log 2>&1
# cmpp runs on the build machine: build it with the native compiler.
make -C src/xspice/cmpp CC=gcc CFLAGS=-O2 LDFLAGS= > /spike/logs/cmpp.log 2>&1
head -c 4 src/xspice/cmpp/cmpp | od -c | head -1 | tee -a /spike/logs/cmpp.log
# Everything else with emcc. The .cm links may fail (no dlopen target); only their objects matter.
emmake make -j"$(nproc)" -k STATIC= libngspice_la_CFLAGS= libngspice_la_LDFLAGS= > $log 2>&1 || echo "make -k reported errors (see build.log)"
lib=src/.libs/libngspice.a
[ -f $lib ] || { grep -E 'error' $log | head -30; exit 1; }

icm=src/xspice/icm
objs=()
for cm in analog digital spice2poly; do
  [ -f $icm/$cm/cminfo.h ] || { echo "no $icm/$cm/cminfo.h"; ls $icm/$cm | head; exit 1; }
  emcc -O2 -I src/include -I $icm/$cm -DPFX=$cm -c /spike/cmreg.c -o $icm/$cm/cmreg.o
  objs+=($icm/$cm/cmreg.o $(find $icm/$cm -name cfunc.o) $(find $icm/$cm -name ifspec.o) $(find $icm/$cm -name udnfunc.o))
done
emcc -O2 -I src/include -c /spike/cmstatic.c -o $icm/cmstatic.o
objs+=($icm/cmstatic.o)
[ -f $icm/msline_common.o ] && objs+=($icm/msline_common.o $icm/tline_common.o)
echo "code-model objects: ${#objs[@]}" | tee /spike/logs/link.log

exports='_ngSpice_Init,_ngSpice_Circ,_ngSpice_Command,_ngGet_Vec_Info,_ngSpice_CurPlot,_ngSpice_AllPlots,_ngSpice_AllVecs,_malloc,_free'
runtime='FS,addFunction,removeFunction,UTF8ToString,stringToUTF8,lengthBytesUTF8,getValue,setValue,HEAPU8,HEAPF64'
em++ -O2 "${objs[@]}" $lib -o /spike/dist/ngspice.mjs \
  -sMODULARIZE=1 -sEXPORT_ES6=1 -sEXPORT_NAME=createNgspice \
  -sENVIRONMENT=web,worker,node -sFORCE_FILESYSTEM=1 -sEXIT_RUNTIME=0 \
  -sALLOW_MEMORY_GROWTH=1 -sINITIAL_MEMORY=33554432 -sSTACK_SIZE=1048576 -sALLOW_TABLE_GROWTH=1 \
  -sEXPORTED_FUNCTIONS="$exports" -sEXPORTED_RUNTIME_METHODS="$runtime" >> /spike/logs/link.log 2>&1 \
  || { tail -40 /spike/logs/link.log; exit 1; }
ls -l /spike/dist
