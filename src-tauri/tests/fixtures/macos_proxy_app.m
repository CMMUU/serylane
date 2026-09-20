// Disposable LaunchServices fixture only. Never linked into the application.
#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        NSApplication *app = [NSApplication sharedApplication];
        [app setActivationPolicy:NSApplicationActivationPolicyAccessory];
        [app finishLaunching];
        NSDictionary *environment = [[NSProcessInfo processInfo] environment];
        NSString *report = environment[@"SERYLANE_FIXTURE_REPORT"];
        NSString *exitFile = environment[@"SERYLANE_FIXTURE_EXIT"];
        if (!report || !exitFile) return 12;
        NSDictionary *payload = @{
            @"httpsProxy": environment[@"HTTPS_PROXY"] ?: @"",
            @"noProxy": environment[@"NO_PROXY"] ?: @"",
            @"arguments": [[NSProcessInfo processInfo] arguments],
            @"bundleId": [[NSBundle mainBundle] bundleIdentifier] ?: @"",
            @"pid": @([[NSProcessInfo processInfo] processIdentifier])
        };
        NSData *json = [NSJSONSerialization dataWithJSONObject:payload options:0 error:nil];
        if (![json writeToFile:report atomically:YES]) return 13;
        NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:30];
        while (![[NSFileManager defaultManager] fileExistsAtPath:exitFile] && [deadline timeIntervalSinceNow] > 0) {
            [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.05]];
        }
        return 0;
    }
}
