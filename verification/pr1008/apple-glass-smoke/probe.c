#include <stdlib.h>
#include <string.h>
extern void apple_glass_probe_start(void);
__attribute__((constructor)) static void start(void) {const char *name=getprogname();if(name && strcmp(name,"apple-glass-smoke")==0) apple_glass_probe_start();}
