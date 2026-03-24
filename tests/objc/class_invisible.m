#include "gnustep_runtime.h"
#include <stdio.h>

int main(void) {
    Class cls = objc_allocateClassPair(NULL, "ClassB", 0);
    printf("%d\n", objc_getClass("ClassB") == NULL);
    objc_registerClassPair(cls);
    printf("%d\n", objc_getClass("ClassB") == cls);
    return 0;
}
