// PROTOTYPE stand-in: what parterre's About parterre item would do on macOS.
#import <AppKit/AppKit.h>
int main(int argc, char **argv) {
    @autoreleasepool {
        NSApplication *app = [NSApplication sharedApplication];
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];
        if (argc > 1)
            app.appearance = [NSAppearance appearanceNamed:(strcmp(argv[1], "--dark") == 0 ? NSAppearanceNameDarkAqua : NSAppearanceNameAqua)];
        dispatch_async(dispatch_get_main_queue(), ^{
            [app activateIgnoringOtherApps:YES];
            // The version parterre knows at runtime (crate::VERSION), commit in parentheses.
            [app orderFrontStandardAboutPanelWithOptions:@{
                NSAboutPanelOptionApplicationVersion: @"0.7.0-rc3",
                NSAboutPanelOptionVersion: @"85656da",
            }];
        });
        [app run];
    }
    return 0;
}
