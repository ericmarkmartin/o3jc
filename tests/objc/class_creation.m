#include "gnustep_runtime.h"
#include <stdio.h>

int main(void) {
    Class cls = objc_allocateClassPair(NULL, "ClassA", 0);
    printf("%d\n", cls != NULL);
    return 0;
}
