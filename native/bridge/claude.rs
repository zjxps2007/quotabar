use quotabar::claude::connection;
use serde::Deserialize;
use std::{
    ffi::c_char,
    io,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Request {
    Locate {
        path: PathBuf,
    },
    Bash,
    Keys,
    Quote {
        text: String,
        bash: bool,
    },
    Load {
        id: String,
    },
    Install {
        directory: PathBuf,
        helper: PathBuf,
        shell: Option<String>,
    },
    Uninstall {
        id: String,
    },
    Fetch {
        directory: PathBuf,
    },
    Latest {
        id: String,
        generation: String,
    },
    Capture {
        id: String,
        generation: String,
    },
}

fn perform(root: &Path, request: Request, input: &[u8]) -> io::Result<Vec<u8>> {
    use serde_json::{json, to_vec};
    Ok(match request {
        Request::Locate { path } => {
            let directory = connection::canonical(&path)?;
            to_vec(&json!({"directory": directory, "id": connection::profile_id(&directory)}))?
        }
        Request::Bash => to_vec(&connection::bash())?,
        Request::Keys => to_vec(connection::API_KEYS)?,
        Request::Quote { text, bash } => to_vec(&connection::quote(&text, bash))?,
        Request::Load { id } => to_vec(&connection::load(root, &id)?)?,
        Request::Install {
            directory,
            helper,
            shell,
        } => to_vec(&connection::install(
            root,
            &directory,
            &helper,
            shell.as_deref(),
        )?)?,
        Request::Uninstall { id } => to_vec(&connection::uninstall(root, &id)?)?,
        Request::Fetch { directory } => to_vec(&connection::fetch(root, &directory)?)?,
        Request::Latest { id, generation } => to_vec(&connection::latest(root, &id, &generation)?)?,
        Request::Capture { id, generation } => {
            connection::capture(root, &id, &generation, input)?;
            to_vec(&true)?
        }
    })
}

/// # Safety
/// Root is a NUL-terminated UTF-8 path. Both buffers are readable for their given
/// lengths. Release the returned JSON or error message with qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_claude(
    root: *const c_char,
    request: *const u8,
    request_len: usize,
    data: *const u8,
    len: usize,
    output: *mut *mut c_char,
) -> i32 {
    unsafe {
        super::windows_operation::invoke(
            root,
            (request, request_len),
            (data, len),
            (4 * 1024 * 1024, 2 * 1024 * 1024),
            output,
            |root, request, data| {
                let result = serde_json::from_slice(request)
                    .map_err(io::Error::from)
                    .and_then(|args| perform(root, args, data));
                result.map_err(|e| {
                    let code = if e.get_ref().is_some_and(|v| v.is::<serde_json::Error>()) {
                        -3
                    } else if e.raw_os_error().is_none() && e.kind() == io::ErrorKind::InvalidInput
                    {
                        -2
                    } else {
                        e.raw_os_error().unwrap_or(-1)
                    };
                    (code, e.to_string())
                })
            },
        )
    }
}
