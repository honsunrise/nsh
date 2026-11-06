#include <stdlib.h>
#include <string.h>
extern void rexnsh_final_probe_start(void);
__attribute__((constructor)) static void load_probe(void) { const char *name=getprogname(); if(name && strcmp(name,"nsh-material-smoke")==0) rexnsh_final_probe_start(); }
