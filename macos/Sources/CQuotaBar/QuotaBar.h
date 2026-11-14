#ifndef QUOTABAR_H
#define QUOTABAR_H
#include <stdint.h>
#include <stddef.h>
typedef struct QBScanner QBScanner;
typedef struct QBCancel QBCancel;
typedef struct QBLogin QBLogin;

QBScanner *qb_scanner_new(void);
void qb_scanner_free(QBScanner *);
void qb_scanner_release_cache(const QBScanner *);
QBCancel *qb_cancel_new(void);
void qb_cancel_free(QBCancel *);
void qb_cancel_set(const QBCancel *);
/* Releases no arguments. Success returns 0 and an owned UTF-8 JSON string.
   Cancellation returns 1. Other failures return -1. */
int32_t qb_scanner_scan(const QBScanner *, const QBCancel *, const char *request, char **output);
void qb_string_free(char *);
char *qb_parse_quota(const char *request);
char *qb_tray_summary(const char *request);
int32_t qb_mac_login_item(int32_t operation);
/* Load returns 0 for success, 1 for a missing credential and -1 on error.
   The returned buffer must be released with its original size. */
int32_t qb_vault_load(const char *directory, const char *uuid, uint8_t **data, size_t *length);
int32_t qb_vault_save(const char *directory, const char *uuid, const uint8_t *data, size_t length);
int32_t qb_vault_delete(const char *directory, const char *uuid);
void qb_secret_free(uint8_t *data, size_t length);
int32_t qb_credentials(const uint8_t *data, size_t length, int32_t provider, int32_t mode, uint8_t **output, size_t *outputLength);
int32_t qb_rotate_credentials(const uint8_t *current, size_t currentLength, const uint8_t *response, size_t responseLength, int64_t nowMicros, uint8_t **output, size_t *outputLength);
char *qb_mac_find_codex(const char *searchPath, const char *home);
char *qb_mac_login_url(const char *text);
QBLogin *qb_mac_login_start(const char *executable, const char *temporaryDirectory, double timeoutSeconds);
int32_t qb_mac_login_poll(QBLogin *attempt, uint8_t **output, size_t *length);
void qb_mac_login_close(QBLogin *attempt, int32_t immediately);
#endif
