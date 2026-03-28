#include <stdio.h>

__attribute__((objc_root_class))
@interface Root
+ (int)rootMethod;
@end

@implementation Root
+ (int)rootMethod { return 1; }
@end

@interface Child : Root
+ (int)childMethod;
@end

@implementation Child
+ (int)childMethod { return 2; }
@end

int main(void) {
    printf("%d\n", [Child rootMethod]);   // 1 — inherited from Root
    printf("%d\n", [Child childMethod]);  // 2 — own method
    printf("%d\n", [Root rootMethod]);    // 1 — direct
    return 0;
}
