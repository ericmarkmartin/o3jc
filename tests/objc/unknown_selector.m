#include "gnustep_runtime.h"
#include <stdio.h>

int main(void) {
    Class cls = objc_allocateClassPair(NULL, "ClassG", 0);
    objc_registerClassPair(cls);
    struct { void *isa; } raw = { cls };
    id obj = (id)&raw;
    IMP imp = objc_msg_lookup(obj, sel_registerName("noSuchMethod"));
    printf("%d\n", imp == NULL);
    return 0;
}
