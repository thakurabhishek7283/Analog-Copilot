/* One code-model library's tables, for linking it statically (P5.0 spike). Compiled once per
   library with -I<lib build dir> and -DPFX=<lib>; replaces dlmain.c's exported tables. */
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include "ngspice/config.h"
#include "ngspice/cpextern.h"
#include "ngspice/devdefs.h"
#include "ngspice/dstring.h"
#include "ngspice/dllitf.h"
#include "ngspice/noisedef.h"
#include "ngspice/evtudn.h"
#include "ngspice/inpdefs.h"
#include "ngspice/inertial.h"
#include "cmextrn.h"
#include "udnextrn.h"
#define CAT2(a, b) a##b
#define CAT(a, b) CAT2(a, b)
SPICEdev *CAT(PFX, _devs)[] = {
#include "cminfo.h"
    NULL
};
int CAT(PFX, _ndevs) = sizeof(CAT(PFX, _devs)) / sizeof(SPICEdev *) - 1;
Evt_Udn_Info_t *CAT(PFX, _udns)[] = {
#include "udninfo.h"
    NULL
};
int CAT(PFX, _nudns) = sizeof(CAT(PFX, _udns)) / sizeof(Evt_Udn_Info_t *) - 1;
