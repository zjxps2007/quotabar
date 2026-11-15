// Loaded only into the two test monitor processes and their home-directory probe.
#import <Foundation/Foundation.h>
#import <LocalAuthentication/LocalAuthentication.h>
#import <Security/Security.h>
#import <objc/runtime.h>
#include <stdlib.h>

static NSString *fixtureHome(void) {
    return [NSString stringWithUTF8String:getenv("QUOTABAR_TEST_MONITOR_HOME")];
}

static NSURL *fixtureHomeURL(id manager, SEL selector) {
    return [NSURL fileURLWithPath:fixtureHome() isDirectory:YES];
}

static NSDictionary *requestJSON(NSURLRequest *request) {
    NSData *data = request.HTTPBody;
    if (!data && request.HTTPBodyStream) {
        NSInputStream *stream = request.HTTPBodyStream;
        NSMutableData *buffer = [NSMutableData data];
        uint8_t chunk[1024];
        [stream open];
        NSInteger n;
        while ((n = [stream read:chunk maxLength:sizeof(chunk)]) > 0 && buffer.length < 8192) {
            [buffer appendBytes:chunk length:(NSUInteger)n];
        }
        [stream close];
        data = buffer;
    }
    return data.length ? [NSJSONSerialization JSONObjectWithData:data options:0 error:NULL] : nil;
}

static BOOL vaultContains(NSString *access) {
    NSString *directory = [fixtureHome() stringByAppendingPathComponent:@"Library/Application Support/AIUsage Lite/Credentials"];
    for (NSString *name in [[NSFileManager defaultManager] contentsOfDirectoryAtPath:directory error:NULL]) {
        if (![name.pathExtension isEqual:@"json"]) continue;
        NSData *data = [NSData dataWithContentsOfFile:[directory stringByAppendingPathComponent:name]];
        NSDictionary *login = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:NULL] : nil;
        if ([login[@"tokens"][@"access_token"] isEqual:access]) return YES;
    }
    return NO;
}

__attribute__((used, section("__DATA,__interpose")))
static const struct { const void *replacement, *original; } interposeHome = {
    (const void *)fixtureHome, (const void *)NSHomeDirectory
};

// During the legacy migration exercise, pass the monitor's own item query to
// Security.framework scoped to one disposable keychain. Never search the
// runner's login keychain for these accounts.
static NSDictionary *migrationQuery(CFDictionaryRef input, NSString *operation) {
    NSDictionary *query = (__bridge NSDictionary *)input;
    const char *path = getenv("QUOTABAR_TEST_KEYCHAIN");
    if (!path || ![query[(__bridge id)kSecAttrService] isEqual:@"com.aiusage.lite.credentials"]) return query;
    SecKeychainRef keychain = NULL;
    if (SecKeychainOpen(path, &keychain) != errSecSuccess) abort();
    NSMutableDictionary *scoped = [query mutableCopy];
    scoped[(__bridge id)kSecMatchSearchList] = @[CFBridgingRelease(keychain)];
    LAContext *context = query[(__bridge id)kSecUseAuthenticationContext];
    Boolean interaction = true;
    BOOL quiet = SecKeychainGetUserInteractionAllowed(&interaction) == errSecSuccess && !interaction && context.interactionNotAllowed;
    NSString *uuid = query[(__bridge id)kSecAttrAccount];
    NSString *root = [fixtureHome() stringByAppendingPathComponent:@"Library/Application Support/AIUsage Lite"];
    NSData *settings = [NSData dataWithContentsOfFile:[root stringByAppendingPathComponent:@"settings.json"]];
    NSDictionary *saved = settings ? [NSJSONSerialization JSONObjectWithData:settings options:0 error:NULL] : nil;
    NSString *vault = [[root stringByAppendingPathComponent:@"Credentials"] stringByAppendingPathComponent:[uuid stringByAppendingString:@".json"]];
    NSDictionary *record = @{@"operation": operation, @"id": uuid, @"quiet": @(quiet),
        @"marker": saved[@"credentialStorageVersion"] ?: @0, @"vault": @([[NSFileManager defaultManager] fileExistsAtPath:vault])};
    NSMutableData *line = [[NSJSONSerialization dataWithJSONObject:record options:0 error:NULL] mutableCopy];
    [line appendBytes:"\n" length:1];
    NSFileHandle *log = [NSFileHandle fileHandleForWritingAtPath:[fixtureHome() stringByAppendingPathComponent:@"legacy-actions.jsonl"]];
    [log seekToEndOfFile]; [log writeData:line]; [log closeFile];
    return scoped;
}
static OSStatus fixtureCopyItem(CFDictionaryRef query, CFTypeRef *result) {
    return SecItemCopyMatching((__bridge CFDictionaryRef)migrationQuery(query, @"read"), result);
}
static OSStatus fixtureDeleteItem(CFDictionaryRef query) {
    return SecItemDelete((__bridge CFDictionaryRef)migrationQuery(query, @"delete"));
}
__attribute__((used, section("__DATA,__interpose")))
static const struct { const void *replacement, *original; } keychainInterposition[] = {
    {(const void *)fixtureCopyItem, (const void *)SecItemCopyMatching},
    {(const void *)fixtureDeleteItem, (const void *)SecItemDelete}
};

// Serve scripted HTTPS responses in both unmodified monitor processes. Only
// fixture requests are accepted; the traces record operation names and statuses.
@interface QuotaBarFixtureRelease : NSURLProtocol
@end
@implementation QuotaBarFixtureRelease
+ (BOOL)canInitWithRequest:(NSURLRequest *)request {
    NSString *scheme = request.URL.scheme;
    return [scheme isEqualToString:@"https"] || [scheme isEqualToString:@"http"];
}
+ (NSURLRequest *)canonicalRequestForRequest:(NSURLRequest *)request { return request; }
- (void)startLoading {
    NSString *endpoint = @"https://api.github.com/repos/zjxps2007/quotabar/releases/latest";
    NSString *url = self.request.URL.absoluteString;
    BOOL release = [url isEqual:endpoint];
    BOOL asset = [url hasPrefix:@"https://github.com/zjxps2007/quotabar/releases/download/"];
    NSString *file = release ? @"release-response.json" : asset ? @"asset-responses.json" : @"provider-responses.json";
    NSData *source = [NSData dataWithContentsOfFile:[fixtureHome() stringByAppendingPathComponent:file]];
    NSDictionary *fixture = source ? [NSJSONSerialization JSONObjectWithData:source options:0 error:NULL] : nil;
    if (release || asset) {
        if (asset) fixture = fixture[url];
        NSString *agent = [self.request valueForHTTPHeaderField:@"User-Agent"];
        BOOL expected = release ? [agent hasPrefix:@"QuotaBar/"] && [[self.request valueForHTTPHeaderField:@"Accept"] isEqual:@"application/vnd.github+json"]
                                : [agent isEqual:@"QuotaBar-Updater"];
        if (!expected || ![self.request.HTTPMethod isEqual:@"GET"] || [self.request valueForHTTPHeaderField:@"Authorization"] ||
            [self.request valueForHTTPHeaderField:@"Cookie"]) fixture = nil;
    } else {
        fixture = fixture[url];
        NSString *authorization = [self.request valueForHTTPHeaderField:@"Authorization"];
        if (fixture[@"byBearer"]) fixture = fixture[@"byBearer"][authorization ?: @""];
        BOOL expected;
        if (fixture[@"refresh"]) {
            NSDictionary *body = requestJSON(self.request);
            expected = !authorization && [body[@"grant_type"] isEqual:@"refresh_token"] &&
                [body[@"client_id"] isEqual:@"app_EMoamEEZ73f0CkXaXp7hrann"] && [body[@"refresh_token"] isEqual:fixture[@"refresh"]];
        } else {
            expected = [authorization isEqual:[@"Bearer " stringByAppendingString:fixture[@"token"] ?: @""]];
        }
        if (!expected || ![self.request.HTTPMethod isEqual:fixture[@"method"]] ||
            (fixture[@"account"] && ![[self.request valueForHTTPHeaderField:@"ChatGPT-Account-Id"] isEqual:fixture[@"account"]]) ||
            (fixture[@"stored"] && !vaultContains(fixture[@"stored"]))) fixture = nil;
    }
    if (![fixture[@"status"] isKindOfClass:NSNumber.class]) {
        [self.client URLProtocol:self didFailWithError:[NSError errorWithDomain:NSURLErrorDomain code:NSURLErrorBadURL userInfo:nil]];
        return;
    }
    NSString *log = [fixtureHome() stringByAppendingPathComponent:(release ? @"release-requests.log" : asset ? @"asset-requests.log" : @"provider-requests.log")];
    NSString *line = release ? [fixture[@"status"] stringValue] : [NSString stringWithFormat:@"%@ %@", fixture[@"name"], fixture[@"status"]];
    @synchronized (QuotaBarFixtureRelease.class) {
        NSFileHandle *handle = [NSFileHandle fileHandleForWritingAtPath:log];
        [handle seekToEndOfFile];
        [handle writeData:[[line stringByAppendingString:@"\n"] dataUsingEncoding:NSUTF8StringEncoding]];
        [handle closeFile];
    }
    NSData *body = asset ? [NSData dataWithContentsOfFile:[[fixtureHome() stringByAppendingPathComponent:@"update-test"] stringByAppendingPathComponent:fixture[@"file"]]]
                         : [NSJSONSerialization dataWithJSONObject:fixture[@"body"] options:0 error:NULL];
    NSHTTPURLResponse *response = [[NSHTTPURLResponse alloc] initWithURL:self.request.URL
        statusCode:[fixture[@"status"] integerValue] HTTPVersion:@"HTTP/1.1"
        headerFields:@{@"Content-Type": asset ? @"application/octet-stream" : @"application/json", @"Content-Length": [@(body.length) stringValue]}];
    [self.client URLProtocol:self didReceiveResponse:response cacheStoragePolicy:NSURLCacheStorageNotAllowed];
    // Exercise more than one data-delegate callback without using the network.
    for (NSUInteger offset = 0; offset < body.length; offset += 8192) {
        [self.client URLProtocol:self didLoadData:[body subdataWithRange:NSMakeRange(offset, MIN(8192, body.length-offset))]];
        // Leave this request open after a real first chunk. The monitor must
        // cancel the session itself when automatic downloads are switched off.
        if (offset == 0 && [fixture[@"holdAfterFirstChunk"] boolValue]) return;
    }
    [self.client URLProtocolDidFinishLoading:self];
}
- (void)stopLoading {}
@end

@interface NSURLSessionConfiguration (QuotaBarFixture)
+ (instancetype)qb_fixtureEphemeralConfiguration;
@end
@implementation NSURLSessionConfiguration (QuotaBarFixture)
+ (instancetype)qb_fixtureEphemeralConfiguration {
    NSURLSessionConfiguration *configuration = [self qb_fixtureEphemeralConfiguration];
    configuration.protocolClasses = [@[QuotaBarFixtureRelease.class] arrayByAddingObjectsFromArray:configuration.protocolClasses ?: @[]];
    return configuration;
}
@end

__attribute__((constructor)) static void useFixtureHome(void) {
    @autoreleasepool {
        if (!getenv("QUOTABAR_TEST_MONITOR_HOME")) abort();
        Method method = class_getInstanceMethod(NSFileManager.class, @selector(homeDirectoryForCurrentUser));
        if (!method) abort();
        method_setImplementation(method, (IMP)fixtureHomeURL);
        Method ephemeral = class_getClassMethod(NSURLSessionConfiguration.class, @selector(ephemeralSessionConfiguration));
        Method fixture = class_getClassMethod(NSURLSessionConfiguration.class, @selector(qb_fixtureEphemeralConfiguration));
        if (!ephemeral || !fixture) abort();
        method_exchangeImplementations(ephemeral, fixture);
    }
}
