/* Static code-model registry and the few helpers dlmain.c defines that the core does not
   (P5.0 spike). load_opus() asks ng_static_cm() before trying dlopen(). */
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "ngspice/config.h"
#include "ngspice/cpextern.h"
#include "ngspice/devdefs.h"
#include "ngspice/dllitf.h"
#include "ngspice/evtudn.h"
#include "ngspice/cmproto.h"
#include "ngspice/inertial.h"

#define LIB(p) extern SPICEdev *p##_devs[]; extern int p##_ndevs; extern Evt_Udn_Info_t *p##_udns[]; extern int p##_nudns;
LIB(analog) LIB(digital) LIB(spice2poly)
static const struct { const char *name; SPICEdev **devs; int *ndevs; Evt_Udn_Info_t **udns; int *nudns; } libs[] = {
    {"analog", analog_devs, &analog_ndevs, analog_udns, &analog_nudns},
    {"digital", digital_devs, &digital_ndevs, digital_udns, &digital_nudns},
    {"spice2poly", spice2poly_devs, &spice2poly_ndevs, spice2poly_udns, &spice2poly_nudns},
};

int ng_static_cm(const char *path, int *n, SPICEdev ***devs, int *nu, Evt_Udn_Info_t ***udns)
{
    const char *base = strrchr(path, '/');
    base = base ? base + 1 : path;
    for (size_t i = 0; i < sizeof libs / sizeof libs[0]; i++) {
        size_t k = strlen(libs[i].name);
        if (strncmp(base, libs[i].name, k) == 0 && (base[k] == '\0' || strcmp(base + k, ".cm") == 0)) {
            *n = *libs[i].ndevs; *devs = libs[i].devs; *nu = *libs[i].nudns; *udns = libs[i].udns;
            return 0;
        }
    }
    return 1;
}

void *malloc_pj(size_t s) { return malloc(s); }
void *calloc_pj(size_t s1, size_t s2) { return calloc(s1, s2); }
void *realloc_pj(const void *ptr, size_t s) { return realloc((void *) ptr, s); }
void free_pj(const void *ptr) { free((void *) ptr); }

int cm_message_printf(const char *fmt, ...)
{
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    return cm_message_send(buf);
}

Mif_Boolean_t cm_is_inertial(enum param_vals param)
{
    int cvar;
    if (cm_getvar("digital_delay_type", CP_NUM, &cvar, sizeof cvar)) {
        if (cvar >= OVERRIDE_TRANSPORT)
            return cvar > OVERRIDE_TRANSPORT;
        if (param == Not_set)
            return cvar != DEFAULT_TRANSPORT;
        return param != Off;
    }
    return param == On;
}

bool cm_getvar(char *name, enum cp_types type, void *retval, size_t rsize)
{
    return cp_getvar(name, type, retval, rsize);
}

/* dlmain.c's search order, shortened: the netlist's directory, then the path as given. */
FILE *fopen_with_path(const char *path, const char *mode)
{
    if (path[0] != '/') {
        const char *x = cm_get_path();
        if (x) {
            char buf[4096];
            snprintf(buf, sizeof buf, "%s/%s", x, path);
            FILE *fp = fopen(buf, mode);
            if (fp)
                return fp;
        }
    }
    return fopen(path, mode);
}
