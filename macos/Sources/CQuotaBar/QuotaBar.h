#ifndef QUOTABAR_H
#define QUOTABAR_H
#include <stdint.h>
#include <stddef.h>
typedef struct { int32_t accounts, local; } QBRefreshDelays;
typedef struct QBMacRefresh QBMacRefresh;
typedef struct { uint8_t bytes[16]; } QBRefreshAccount;
typedef struct { QBRefreshAccount account; uint64_t generation; } QBRefreshJob;
typedef struct { size_t queued, running; int32_t sleeping, installing; } QBMacRefreshStatus;
QBMacRefresh *qb_mac_refresh_new(void);
void qb_mac_refresh_free(QBMacRefresh *);
void qb_mac_refresh_enqueue(QBMacRefresh *, QBRefreshAccount);
QBRefreshJob qb_mac_refresh_next(QBMacRefresh *);
int32_t qb_mac_refresh_finish(QBMacRefresh *, QBRefreshJob);
void qb_mac_refresh_cancel(QBMacRefresh *, QBRefreshAccount);
void qb_mac_refresh_sleeping(QBMacRefresh *, int32_t);
void qb_mac_refresh_installing(QBMacRefresh *, int32_t);
QBMacRefreshStatus qb_mac_refresh_status(const QBMacRefresh *, void *, void (*visitor)(void *, QBRefreshAccount));
QBRefreshDelays qb_refresh_delays(int32_t suspended, int32_t automatic, int32_t accounts, int32_t dashboard,
                                 int32_t logs, int32_t scanning, int32_t interval);
int32_t qb_refresh_interval(size_t index);
int32_t qb_local_interval(void);
int32_t qb_private_write(const char *path, const uint8_t *data, size_t length);
int32_t qb_mac_claude(const char *root, const uint8_t *request, size_t length, char **output);
int32_t qb_mac_settings(const char *path, const char *vault, const uint8_t *request, size_t request_length,
                        const uint8_t *credential, size_t credential_length, char **output);
/* Successful selection: stored (0), refreshed CLI login (1), or stored after
   a CLI account switch (2). Release the returned bytes with qb_secret_free. */
int32_t qb_mac_imported(const char *vault, const uint8_t *account, size_t accountLength, uint8_t **output, size_t *length);
int32_t qb_mac_snapshot_parse(const char *id, const char *generation, double now, const uint8_t *input, size_t length, char **output);
char *qb_mac_snapshot_latest(const char *folder, const char *id, const char *generation, double now);
int32_t qb_mac_snapshot_save(const char *folder, const uint8_t *data, size_t length);
typedef struct QBScanner QBScanner;
typedef struct QBCancel QBCancel;
typedef struct QBLogin QBLogin;
typedef struct QBFrameDecoder QBFrameDecoder;
int32_t qb_mac_monitor_run(void *context, void (*event)(void *, int32_t));
typedef struct QBMacInstaller QBMacInstaller;
typedef struct QBUpdatePolicy QBUpdatePolicy;
typedef enum { QBUpdateSchedule, QBUpdateCancel, QBUpdateInstall } QBUpdateOnConfigure;
typedef enum { QBUpdateCurrent, QBUpdateBlocked, QBUpdateAvailable } QBUpdateOffer;
QBUpdatePolicy *qb_update_policy_new(void);
void qb_update_policy_free(QBUpdatePolicy *);
QBUpdateOnConfigure qb_update_configure(QBUpdatePolicy *, int32_t automatic, int32_t canInstall, int32_t suspended);
int32_t qb_update_begin_check(QBUpdatePolicy *, int32_t manual);
void qb_update_attempted(QBUpdatePolicy *, double now);
QBUpdateOffer qb_update_offer(const QBUpdatePolicy *, const char *current, const char *version);
int32_t qb_update_should_download(const QBUpdatePolicy *, int32_t requested);
void qb_update_staged(QBUpdatePolicy *, const char *version);
void qb_update_block(QBUpdatePolicy *, const char *version);
int32_t qb_update_begin_install(QBUpdatePolicy *, int32_t force);
int32_t qb_update_begin_handoff(QBUpdatePolicy *, int32_t force);
int32_t qb_update_may_install(const QBUpdatePolicy *, int32_t force);
void qb_update_finish(QBUpdatePolicy *, int32_t cancelled);
double qb_update_delay(const QBUpdatePolicy *, double now);
int32_t qb_mac_update_prepare(const uint8_t *plan, size_t length);
void qb_mac_update_discard(const char *stage, const char *target);
QBMacInstaller *qb_mac_update_launch(const char *job);
/* A live helper returns 0 while waiting and 1 once the intended version is ready. */
int32_t qb_mac_update_ready(QBMacInstaller *);
void qb_mac_update_finish(QBMacInstaller *, int32_t terminate);
QBFrameDecoder *qb_frame_decoder_new(void);
QBFrameDecoder *qb_frame_decoder_copy(const QBFrameDecoder *);
void qb_frame_decoder_free(QBFrameDecoder *);
int32_t qb_frame_append(QBFrameDecoder *, const uint8_t *, size_t, void (*deliver)(void *, const uint8_t *, size_t), void *);
int32_t qb_frame_finish(const QBFrameDecoder *);
int32_t qb_frame_encode(const uint8_t *, size_t, uint8_t **output, size_t *outputLength);
typedef struct QBMacDashboardHost QBMacDashboardHost;
typedef enum : int32_t {
    QBHostStarted, QBHostExited, QBHostMissingHelper, QBHostLaunchFailed, QBHostPublishState, QBHostCommand
} QBMacHostEvent;
typedef int32_t (*QBMacHostCallback)(void *, QBMacHostEvent, uint32_t, const uint8_t *, size_t);
const QBMacDashboardHost *qb_mac_dashboard_host_new(void *, QBMacHostCallback, int32_t demo);
void qb_mac_dashboard_host_show(const QBMacDashboardHost *, const char *route);
void qb_mac_dashboard_host_send(const QBMacDashboardHost *, const uint8_t *, size_t);
void qb_mac_dashboard_host_changed(const QBMacDashboardHost *);
void qb_mac_dashboard_host_close(const QBMacDashboardHost *);
int32_t qb_mac_dashboard_host_running(const QBMacDashboardHost *);
void qb_mac_dashboard_host_free(const QBMacDashboardHost *);

QBScanner *qb_scanner_new(void);
void qb_scanner_free(QBScanner *);
void qb_scanner_release_cache(const QBScanner *);
QBCancel *qb_cancel_new(void);
void qb_cancel_free(QBCancel *);
void qb_cancel_set(const QBCancel *);
int32_t qb_release_version(const uint8_t *, size_t, int32_t components[3]);
char *qb_release_feed(int32_t platform, const uint8_t *, size_t);
/* Both calls run on a worker. Release returned JSON with qb_string_free.
   0 success, 1..5 public release errors, 6 cancelled, 7 I/O, 1000+errno POSIX,
   negative numbers are Foundation URL error codes. */
int32_t qb_mac_release_latest(int32_t platform, const char *current, const QBCancel *, char **json);
int32_t qb_mac_release_stage(const uint8_t *metadata, size_t length, const char *directory,
                             const QBCancel *, void *context, void (*progress)(void *, int32_t), char **json);
int32_t qb_release_verify(const uint8_t *metadata, size_t metadataLength, const uint8_t *manifest, size_t manifestLength, const uint8_t *signature, size_t signatureLength);
int32_t qb_release_file(const char *path, const uint8_t *asset, size_t length, const QBCancel *);
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
typedef struct QBCodexSessions QBCodexSessions;
typedef struct QBSessionRead QBSessionRead;
/* A non-null receiver requests a load. Supply its data with qb_sessions_loaded,
 * return 1 when missing, or a negative code on failure. A null receiver requests a save. */
typedef struct {
    void *context;
    int32_t (*access)(void *context, const char *uuid, const uint8_t *value, size_t length, QBSessionRead *receiver);
} QBSessionStore;
QBCodexSessions *qb_sessions_new(int32_t mac, const char *directory);
void qb_sessions_free(QBCodexSessions *);
int32_t qb_sessions_loaded(QBSessionRead *, const uint8_t *value, size_t length);
/* Begin returns 0 for ready, 1 for a new rotation, 2 to join an existing ticket.
 * Release an owned output with qb_secret_free. A null store uses the vault directory. */
int32_t qb_sessions_begin(const QBCodexSessions *, const char *uuid, const uint8_t *identity, size_t identityLength,
                          const uint8_t *rejected, size_t rejectedLength, const QBSessionStore *,
                          uint64_t *ticket, uint8_t **output, size_t *length);
int32_t qb_sessions_complete(const QBCodexSessions *, const char *uuid, uint64_t ticket,
                             const uint8_t *replacement, size_t length, const QBSessionStore *);
int32_t qb_sessions_flush(const QBCodexSessions *, const QBSessionStore *);
/* Invalidating (0) returns the detached ticket or zero. Revoke (1) and end (2) apply only to the supplied ticket. */
uint64_t qb_sessions_event(const QBCodexSessions *, const char *uuid, uint64_t ticket, int32_t event);
typedef struct QBProviderQuery QBProviderQuery;
/* A positive result is the next HTTP request (1 optional, 2 required). Zero is
 * the final result. -30 carries a provider error. Release buffers with qb_secret_free. */
int32_t qb_provider_query_start(int32_t op, int32_t mac, const uint8_t *auth, size_t authLength, double timeout,
                                QBProviderQuery **query, uint8_t **output, size_t *outputLength);
int32_t qb_provider_query_step(QBProviderQuery *query, int32_t status, const uint8_t *body, size_t length,
                               int64_t nowMicros, uint8_t **output, size_t *outputLength);
void qb_provider_query_free(QBProviderQuery *query);
int32_t qb_provider_query_run(int32_t op, const uint8_t *auth, size_t authLength, double timeout,
                             int64_t nowMicros, const QBCancel *cancel, int32_t *osError, uint8_t **output, size_t *outputLength);
int32_t qb_rotate_credentials(const uint8_t *current, size_t currentLength, const uint8_t *response, size_t responseLength, int64_t nowMicros, uint8_t **output, size_t *outputLength);
char *qb_mac_find_codex(const char *searchPath, const char *home);
char *qb_mac_login_url(const char *text);
QBLogin *qb_mac_login_start(const char *executable, const char *temporaryDirectory, double timeoutSeconds);
int32_t qb_mac_login_poll(QBLogin *attempt, uint8_t **output, size_t *length);
void qb_mac_login_close(QBLogin *attempt, int32_t immediately);
typedef struct QBMacStatus QBMacStatus;
void qb_mac_appearance(int32_t mode);
int32_t qb_mac_dock_visible(int32_t visible);
QBMacStatus *qb_mac_status_new(void *context, void (*callback)(void *, int32_t));
typedef enum {
    QBMacStatusRefresh = 1, QBMacStatusDashboard = 2, QBMacStatusSettings = 3,
    QBMacStatusClosed = 5, QBMacStatusOpened = 7
} QBMacStatusEvent;
QBMacStatus *qb_mac_monitor_status_new(void *context, void (*callback)(void *, QBMacStatusEvent));
void qb_mac_status_show_popover(const QBMacStatus *);
void qb_mac_status_state(const QBMacStatus *, const uint8_t *, size_t);
int32_t qb_mac_status_publish(const QBMacStatus *, const uint8_t *, size_t);
int32_t qb_mac_status_control(const QBMacStatus *, int32_t);
void qb_mac_status_free(QBMacStatus *);
#endif
