use std::{
    error::Error,
    fmt::{Display, Formatter},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use android_activity::AndroidApp;
use jni::{
    JValue, JavaVM, jni_sig, jni_str,
    objects::{JByteArray, JObject, JString},
    sys::jobject,
};

use crate::model::DeviceAudio;

const MEDIA_STORE_AUDIO_URI: &str = "content://media/external/audio/media";
const MEDIA_PERMISSION_REQUEST: i32 = 4201;
const WRITE_PERMISSION_REQUEST: i32 = 4202;
const FOLDER_PICKER_REQUEST: i32 = 4203;

#[derive(Debug)]
enum AndroidError {
    Jni(jni::errors::Error),
    Io(std::io::Error),
    Message(String),
}

impl Display for AndroidError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Jni(error) => write!(formatter, "{error}"),
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl Error for AndroidError {}

impl From<jni::errors::Error> for AndroidError {
    fn from(error: jni::errors::Error) -> Self {
        Self::Jni(error)
    }
}

impl From<std::io::Error> for AndroidError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone)]
pub(crate) struct AndroidBridge {
    app: AndroidApp,
    vm: JavaVM,
}

impl AndroidBridge {
    pub(crate) fn new(app: AndroidApp) -> Self {
        let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) };
        Self { app, vm }
    }

    fn with_env<T>(
        &self,
        callback: impl FnOnce(&mut jni::Env<'_>) -> Result<T, AndroidError>,
    ) -> Result<T, String> {
        self.vm
            .attach_current_thread(callback)
            .map_err(|error| error.to_string())
    }

    fn activity<'local>(&self, env: &mut jni::Env<'local>) -> JObject<'local> {
        unsafe { JObject::from_raw(env, self.app.activity_as_ptr() as jobject) }
    }

    pub(crate) fn sdk_int(&self) -> i32 {
        self.with_env(|env| {
            Ok(env
                .get_static_field(
                    jni_str!("android/os/Build$VERSION"),
                    jni_str!("SDK_INT"),
                    jni_sig!("I"),
                )?
                .i()?)
        })
        .unwrap_or(26)
    }

    /// egui "points" per physical pixel, mirroring what winit computes from
    /// the configuration density (`AConfiguration_getDensity() / 160`).
    pub(crate) fn pixels_per_point(&self) -> f32 {
        self.app
            .config()
            .density()
            .map(|dpi| dpi as f32 / 160.0)
            .unwrap_or(1.0)
            .max(1.0)
    }

    /// Hit-test-safe screen padding: how much of the window is covered by
    /// the status bar, gesture/3-button navigation bar, and display cutout,
    /// in egui points. NativeActivity draws edge-to-edge, so the UI would
    /// otherwise sit underneath those system bars.
    pub(crate) fn window_insets(&self) -> (f32, f32, f32, f32) {
        let scale = self.pixels_per_point();
        let px = self.system_window_insets_px().unwrap_or_default();
        (
            (px.0 as f32 / scale).max(0.0),
            (px.1 as f32 / scale).max(0.0),
            (px.2 as f32 / scale).max(0.0),
            (px.3 as f32 / scale).max(0.0),
        )
    }

    /// System window insets in pixels via the legacy
    /// `WindowInsets#getSystemWindowInset*()` getters (API 23+, still
    /// functional and covers system bars + cutout for our minSdk 26).
    fn system_window_insets_px(&self) -> Result<(i32, i32, i32, i32), String> {
        self.with_env(|env| {
            let activity = self.activity(env);
            let window = env
                .call_method(
                    &activity,
                    jni_str!("getWindow"),
                    jni_sig!("()Landroid/view/Window;"),
                    &[],
                )?
                .l()?;
            let decor = env
                .call_method(
                    &window,
                    jni_str!("getDecorView"),
                    jni_sig!("()Landroid/view/View;"),
                    &[],
                )?
                .l()?;
            let insets = env
                .call_method(
                    &decor,
                    jni_str!("getRootWindowInsets"),
                    jni_sig!("()Landroid/view/WindowInsets;"),
                    &[],
                )?
                .l()?;
            if insets.is_null() {
                // View not attached yet this early: report nothing rather
                // than pretending the whole screen is obscured.
                return Ok((0, 0, 0, 0));
            }
            let top = env
                .call_method(
                    &insets,
                    jni_str!("getSystemWindowInsetTop"),
                    jni_sig!("()I"),
                    &[],
                )?
                .i()?;
            let bottom = env
                .call_method(
                    &insets,
                    jni_str!("getSystemWindowInsetBottom"),
                    jni_sig!("()I"),
                    &[],
                )?
                .i()?;
            let left = env
                .call_method(
                    &insets,
                    jni_str!("getSystemWindowInsetLeft"),
                    jni_sig!("()I"),
                    &[],
                )?
                .i()?;
            let right = env
                .call_method(
                    &insets,
                    jni_str!("getSystemWindowInsetRight"),
                    jni_sig!("()I"),
                    &[],
                )?
                .i()?;
            Ok((top, bottom, left, right))
        })
        .map_err(|error| format!("Could not read window insets: {error}"))
    }

    pub(crate) fn files_dir(&self) -> Result<PathBuf, String> {
        self.with_env(|env| {
            let activity = self.activity(env);
            let files = env
                .call_method(
                    &activity,
                    jni_str!("getFilesDir"),
                    jni_sig!("()Ljava/io/File;"),
                    &[],
                )?
                .l()?;
            let path = env
                .call_method(
                    &files,
                    jni_str!("getAbsolutePath"),
                    jni_sig!("()Ljava/lang/String;"),
                    &[],
                )?
                .l()?;
            Ok(PathBuf::from(java_string(env, path)?))
        })
    }

    pub(crate) fn cache_dir(&self) -> Result<PathBuf, String> {
        self.with_env(|env| {
            let activity = self.activity(env);
            let cache = env
                .call_method(
                    &activity,
                    jni_str!("getCacheDir"),
                    jni_sig!("()Ljava/io/File;"),
                    &[],
                )?
                .l()?;
            let path = env
                .call_method(
                    &cache,
                    jni_str!("getAbsolutePath"),
                    jni_sig!("()Ljava/lang/String;"),
                    &[],
                )?
                .l()?;
            Ok(PathBuf::from(java_string(env, path)?))
        })
    }

    pub(crate) fn has_media_permission(&self) -> bool {
        let permission = self.media_permission();
        self.has_permission(&permission)
    }

    pub(crate) fn request_media_permission(&self) -> Result<(), String> {
        self.request_permissions(&[self.media_permission()], MEDIA_PERMISSION_REQUEST)
    }

    pub(crate) fn has_write_permission(&self) -> bool {
        self.sdk_int() >= 29 || self.has_permission("android.permission.WRITE_EXTERNAL_STORAGE")
    }

    pub(crate) fn request_write_permission(&self) -> Result<(), String> {
        if self.sdk_int() >= 29 {
            return Ok(());
        }
        self.request_permissions(
            &["android.permission.WRITE_EXTERNAL_STORAGE".to_owned()],
            WRITE_PERMISSION_REQUEST,
        )
    }

    /// Launch the system folder picker (Storage Access Framework)
    /// to let the user choose a directory containing music files.
    pub(crate) fn pick_folder(&self) -> Result<(), String> {
        self.with_env(|env| {
            let activity = self.activity(env);
            let action = env.new_string("android.intent.action.OPEN_DOCUMENT_TREE")?;
            let intent = env.new_object(
                jni_str!("android/content/Intent"),
                jni_sig!("(Ljava/lang/String;)V"),
                &[JValue::Object(&action.into())],
            )?;
            // Allow picking directories only
            let flags = env
                .get_static_field(
                    jni_str!("android/content/Intent"),
                    jni_str!("FLAG_GRANT_READ_URI_PERMISSION"),
                    jni_sig!("I"),
                )?
                .i()?
                | env
                    .get_static_field(
                        jni_str!("android/content/Intent"),
                        jni_str!("FLAG_GRANT_PERSISTABLE_URI_PERMISSION"),
                        jni_sig!("I"),
                    )?
                    .i()?;
            env.call_method(
                &intent,
                jni_str!("addFlags"),
                jni_sig!("(I)Landroid/content/Intent;"),
                &[JValue::Int(flags)],
            )?;
            // Start the activity for result
            env.call_method(
                &activity,
                jni_str!("startActivityForResult"),
                jni_sig!("(Landroid/content/Intent;I)V"),
                &[JValue::Object(&intent), JValue::Int(FOLDER_PICKER_REQUEST)],
            )?;
            Ok(())
        })
    }

    fn media_permission(&self) -> String {
        if self.sdk_int() >= 33 {
            "android.permission.READ_MEDIA_AUDIO".to_owned()
        } else {
            "android.permission.READ_EXTERNAL_STORAGE".to_owned()
        }
    }

    fn has_permission(&self, permission: &str) -> bool {
        self.with_env(|env| {
            let activity = self.activity(env);
            let permission = JString::new(env, permission)?;
            let result = env.call_method(
                &activity,
                jni_str!("checkSelfPermission"),
                jni_sig!("(Ljava/lang/String;)I"),
                &[JValue::Object(&permission)],
            )?;
            Ok(result.i()? == 0)
        })
        .unwrap_or(false)
    }

    fn request_permissions(&self, permissions: &[String], request_code: i32) -> Result<(), String> {
        self.with_env(|env| {
            let activity = self.activity(env);
            let array = env.new_object_array(
                permissions.len() as i32,
                jni_str!("java/lang/String"),
                JObject::null(),
            )?;
            for (index, permission) in permissions.iter().enumerate() {
                let permission = JString::new(env, permission)?;
                env.set_object_array_element(&array, index, &permission)?;
            }
            env.call_method(
                &activity,
                jni_str!("requestPermissions"),
                jni_sig!("([Ljava/lang/String;I)V"),
                &[JValue::Object(&array), JValue::Int(request_code)],
            )?;
            Ok(())
        })
    }

    pub(crate) fn query_device_audio(&self) -> Result<Vec<DeviceAudio>, String> {
        self.with_env(|env| {
            let activity = self.activity(env);
            let resolver = env
                .call_method(
                    &activity,
                    jni_str!("getContentResolver"),
                    jni_sig!("()Landroid/content/ContentResolver;"),
                    &[],
                )?
                .l()?;
            let collection = JString::new(env, MEDIA_STORE_AUDIO_URI)?;
            let uri = env.call_static_method(
                jni_str!("android/net/Uri"),
                jni_str!("parse"),
                jni_sig!("(Ljava/lang/String;)Landroid/net/Uri;"),
                &[JValue::Object(&collection)],
            )?
            .l()?;
            // Canonical MediaStore column names. The provider's actual SQLite
            // columns are `_id` and `_display_name`; `DISPLAY_NAME` is the
            // *constant name*, not the column, and querying it throws
            // "invalid column display_name" from MediaProvider.
            let projection = env.new_object_array(
                7,
                jni_str!("java/lang/String"),
                JObject::null(),
            )?;
            for (index, column) in [
                "_id",
                "title",
                "artist",
                "album",
                "duration",
                "mime_type",
                "_display_name",
            ]
            .into_iter()
            .enumerate()
            {
                let column = JString::new(env, column)?;
                env.set_object_array_element(
                    &projection,
                    index,
                    &column,
                )?;
            }
            let selection = JString::new(env, "TITLE ASC")?;
            let null = JObject::null();
            let cursor = env.call_method(
                &resolver,
                jni_str!("query"),
                jni_sig!("(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;"),
                &[
                    JValue::Object(&uri),
                    JValue::Object(&projection),
                    JValue::Object(&null),
                    JValue::Object(&null),
                    JValue::Object(&selection),
                ],
            )?
            .l()?;

            let result = (|| -> Result<Vec<DeviceAudio>, AndroidError> {
                let count = env
                    .call_method(&cursor, jni_str!("getCount"), jni_sig!("()I"), &[])?
                    .i()?;
                if count <= 0
                    || !env
                        .call_method(&cursor, jni_str!("moveToFirst"), jni_sig!("()Z"), &[])?
                        .z()?
                {
                    return Ok(Vec::new());
                }
                let mut tracks = Vec::new();
                for index in 0..count {
                    let id = env
                        .call_method(
                            &cursor,
                            jni_str!("getLong"),
                            jni_sig!("(I)J"),
                            &[JValue::Int(0)],
                        )?
                        .j()?;
                    let title = cursor_string(env, &cursor, 1)?;
                    let artist = cursor_string(env, &cursor, 2)?;
                    let album = cursor_string(env, &cursor, 3)?;
                    let duration_ms = env
                        .call_method(
                            &cursor,
                            jni_str!("getLong"),
                            jni_sig!("(I)J"),
                            &[JValue::Int(4)],
                        )?
                        .j()?;
                    let mime_type = cursor_string(env, &cursor, 5)?.to_ascii_lowercase();
                    let display_name = cursor_string(env, &cursor, 6)?;
                    if is_supported_audio(&mime_type, &display_name) {
                        tracks.push(DeviceAudio {
                            id,
                            title: clean_media_value(&title, "Unknown title"),
                            artist: clean_media_value(&artist, "Unknown artist"),
                            album: clean_media_value(&album, "Local library"),
                            duration_ms,
                            mime_type,
                            display_name,
                        });
                    }
                    if index + 1 < count {
                        env.call_method(&cursor, jni_str!("moveToNext"), jni_sig!("()Z"), &[])?
                            .z()?;
                    }
                }
                Ok(tracks)
            })();
            let _ = env.call_method(&cursor, jni_str!("close"), jni_sig!("()V"), &[]);
            result
        })
    }

    /// Query audio files from a specific directory URI (from folder picker).
    /// Uses the Storage Access Framework to scan for audio files recursively.
    pub(crate) fn query_audio_from_folder(
        &self,
        folder_uri: &str,
    ) -> Result<Vec<DeviceAudio>, String> {
        self.with_env(|env| {
            let activity = self.activity(env);
            let resolver = env
                .call_method(
                    &activity,
                    jni_str!("getContentResolver"),
                    jni_sig!("()Landroid/content/ContentResolver;"),
                    &[],
                )?
                .l()?;
            let folder_uri_jstring = env.new_string(folder_uri)?;
            let uri = env.call_static_method(
                jni_str!("android/net/Uri"),
                jni_str!("parse"),
                jni_sig!("(Ljava/lang/String;)Landroid/net/Uri;"),
                &[JValue::Object(&folder_uri_jstring.into())],
            )?
            .l()?;
            
            // Take persistable URI permission
            env.call_method(
                &resolver,
                jni_str!("takePersistableUriPermission"),
                jni_sig!("(Landroid/net/Uri;I)V"),
                &[
                    JValue::Object(&uri),
                    JValue::Int(1), // FLAG_GRANT_READ_URI_PERMISSION
                ],
            )?;

            // Query for audio files under the selected directory
            // Using DocumentsContract to query children
            let documents_contract = jni_str!("android/provider/DocumentsContract");
            let docs_str = env.new_string("documents")?;
            let children_uri = env.call_static_method(
                documents_contract,
                jni_str!("buildChildDocumentsUriUsingTree"),
                jni_sig!("(Landroid/net/Uri;Ljava/lang/String;)Landroid/net/Uri;"),
                &[
                    JValue::Object(&uri),
                    JValue::Object(&docs_str.into()),
                ],
            )?
            .l()?;

            // Canonical MediaStore column names
            let projection = env.new_object_array(
                7,
                jni_str!("java/lang/String"),
                JObject::null(),
            )?;
            for (index, column) in [
                "_id",
                "title",
                "artist",
                "album",
                "duration",
                "mime_type",
                "_display_name",
            ]
            .into_iter()
            .enumerate()
            {
                let column = JString::new(env, column)?;
                env.set_object_array_element(&projection, index, &column)?;
            }

            let selection = JString::new(env, "mime_type LIKE 'audio/%'")?;
            let null = JObject::null();
            let cursor = env.call_method(
                &resolver,
                jni_str!("query"),
                jni_sig!("(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;"),
                &[
                    JValue::Object(&children_uri),
                    JValue::Object(&projection),
                    JValue::Object(&null),
                    JValue::Object(&null),
                    JValue::Object(&selection),
                ],
            )?
            .l()?;

            let result = (|| -> Result<Vec<DeviceAudio>, AndroidError> {
                let count = env
                    .call_method(&cursor, jni_str!("getCount"), jni_sig!("()I"), &[])?
                    .i()?;
                if count <= 0
                    || !env
                        .call_method(&cursor, jni_str!("moveToFirst"), jni_sig!("()Z"), &[])?
                        .z()?
                {
                    return Ok(Vec::new());
                }
                let mut tracks = Vec::new();
                for index in 0..count {
                    let id = env
                        .call_method(
                            &cursor,
                            jni_str!("getLong"),
                            jni_sig!("(I)J"),
                            &[JValue::Int(0)],
                        )?
                        .j()?;
                    let title = cursor_string(env, &cursor, 1)?;
                    let artist = cursor_string(env, &cursor, 2)?;
                    let album = cursor_string(env, &cursor, 3)?;
                    let duration_ms = env
                        .call_method(
                            &cursor,
                            jni_str!("getLong"),
                            jni_sig!("(I)J"),
                            &[JValue::Int(4)],
                        )?
                        .j()?;
                    let mime_type = cursor_string(env, &cursor, 5)?.to_ascii_lowercase();
                    let display_name = cursor_string(env, &cursor, 6)?;
                    if is_supported_audio(&mime_type, &display_name) {
                        tracks.push(DeviceAudio {
                            id,
                            title: clean_media_value(&title, "Unknown title"),
                            artist: clean_media_value(&artist, "Unknown artist"),
                            album: clean_media_value(&album, "Local library"),
                            duration_ms,
                            mime_type,
                            display_name,
                        });
                    }
                    if index + 1 < count {
                        env.call_method(&cursor, jni_str!("moveToNext"), jni_sig!("()Z"), &[])?
                            .z()?;
                    }
                }
                Ok(tracks)
            })();
            let _ = env.call_method(&cursor, jni_str!("close"), jni_sig!("()V"), &[]);
            result
        })
    }

    pub(crate) fn import_audio(&self, item: &DeviceAudio) -> Result<PathBuf, String> {
        self.with_env(|env| {
            let activity = self.activity(env);
            let resolver = env
                .call_method(
                    &activity,
                    jni_str!("getContentResolver"),
                    jni_sig!("()Landroid/content/ContentResolver;"),
                    &[],
                )?
                .l()?;
            let collection = JString::new(env, &format!("{MEDIA_STORE_AUDIO_URI}/{}", item.id))?;
            let uri = env
                .call_static_method(
                    jni_str!("android/net/Uri"),
                    jni_str!("parse"),
                    jni_sig!("(Ljava/lang/String;)Landroid/net/Uri;"),
                    &[JValue::Object(&collection)],
                )?
                .l()?;
            let stream = env
                .call_method(
                    &resolver,
                    jni_str!("openInputStream"),
                    jni_sig!("(Landroid/net/Uri;)Ljava/io/InputStream;"),
                    &[JValue::Object(&uri)],
                )?
                .l()?;
            if stream.is_null() {
                return Err(AndroidError::Message(
                    "Android could not open that audio file.".to_owned(),
                ));
            }

            let files_dir = self.files_dir_in_env(env)?;
            let media_dir = files_dir.join("media");
            fs::create_dir_all(&media_dir)?;
            let extension = audio_extension(&item.mime_type, &item.display_name);
            let destination = media_dir.join(format!("media-{}.{extension}", item.id));
            let mut output = File::create(&destination)?;
            let buffer = JByteArray::new(env, 64 * 1024)?;
            let mut signed_bytes = vec![0i8; 64 * 1024];
            loop {
                let read = env
                    .call_method(
                        &stream,
                        jni_str!("read"),
                        jni_sig!("([B)I"),
                        &[JValue::Object(&buffer)],
                    )?
                    .i()?;
                if read < 0 {
                    break;
                }
                if read == 0 {
                    continue;
                }
                let read = read as usize;
                buffer.get_region(env, 0, &mut signed_bytes[..read])?;
                let bytes =
                    unsafe { std::slice::from_raw_parts(signed_bytes.as_ptr().cast::<u8>(), read) };
                output.write_all(bytes)?;
            }
            let _ = env.call_method(&stream, jni_str!("close"), jni_sig!("()V"), &[]);
            output.sync_all()?;
            Ok(destination)
        })
    }

    pub(crate) fn export_wav(&self, source: &Path, display_name: &str) -> Result<String, String> {
        let sdk_int = self.sdk_int();
        self.with_env(|env| {
            if sdk_int >= 29 {
                self.export_wav_scoped(env, source, display_name)
            } else {
                self.export_wav_legacy(env, source, display_name)
            }
        })
    }

    fn export_wav_scoped(
        &self,
        env: &mut jni::Env<'_>,
        source: &Path,
        display_name: &str,
    ) -> Result<String, AndroidError> {
        let activity = self.activity(env);
        let resolver = env
            .call_method(
                &activity,
                jni_str!("getContentResolver"),
                jni_sig!("()Landroid/content/ContentResolver;"),
                &[],
            )?
            .l()?;
        let collection_string = JString::new(env, MEDIA_STORE_AUDIO_URI)?;
        let collection = env
            .call_static_method(
                jni_str!("android/net/Uri"),
                jni_str!("parse"),
                jni_sig!("(Ljava/lang/String;)Landroid/net/Uri;"),
                &[JValue::Object(&collection_string)],
            )?
            .l()?;
        let values = env.new_object(
            jni_str!("android/content/ContentValues"),
            jni_sig!("()V"),
            &[],
        )?;
        content_values_put(env, &values, "DISPLAY_NAME", display_name)?;
        content_values_put(env, &values, "MIME_TYPE", "audio/x-wav")?;
        content_values_put(env, &values, "RELATIVE_PATH", "Music/OpenMix")?;
        content_values_put_int(env, &values, "IS_MUSIC", 1)?;
        let uri = env
            .call_method(
                &resolver,
                jni_str!("insert"),
                jni_sig!("(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;"),
                &[JValue::Object(&collection), JValue::Object(&values)],
            )?
            .l()?;
        if uri.is_null() {
            return Err(AndroidError::Message(
                "Android could not create the export file.".to_owned(),
            ));
        }
        let mode = JString::new(env, "w")?;
        let output = env
            .call_method(
                &resolver,
                jni_str!("openOutputStream"),
                jni_sig!("(Landroid/net/Uri;Ljava/lang/String;)Ljava/io/OutputStream;"),
                &[JValue::Object(&uri), JValue::Object(&mode)],
            )?
            .l()?;
        if let Err(error) = copy_file_to_java(env, source, &output) {
            let null = JObject::null();
            let _ = env.call_method(
                &resolver,
                jni_str!("delete"),
                jni_sig!("(Landroid/net/Uri;Ljava/lang/String;[Ljava/lang/String;)I"),
                &[
                    JValue::Object(&uri),
                    JValue::Object(&null),
                    JValue::Object(&null),
                ],
            );
            return Err(error);
        }
        Ok(format!("Music/OpenMix/{display_name}"))
    }

    fn export_wav_legacy(
        &self,
        env: &mut jni::Env<'_>,
        source: &Path,
        display_name: &str,
    ) -> Result<String, AndroidError> {
        let music_name = JString::new(env, "Music")?;
        let music = env
            .call_static_method(
                jni_str!("android/os/Environment"),
                jni_str!("getExternalStoragePublicDirectory"),
                jni_sig!("(Ljava/lang/String;)Ljava/io/File;"),
                &[JValue::Object(&music_name)],
            )?
            .l()?;
        let root = env
            .call_method(
                &music,
                jni_str!("getAbsolutePath"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        let root = PathBuf::from(java_string(env, root)?);
        let directory = root.join("OpenMix");
        fs::create_dir_all(&directory)?;
        let destination = directory.join(display_name);
        fs::copy(source, &destination)?;
        Ok(destination.to_string_lossy().into_owned())
    }

    fn files_dir_in_env(&self, env: &mut jni::Env<'_>) -> Result<PathBuf, AndroidError> {
        let activity = self.activity(env);
        let files = env
            .call_method(
                &activity,
                jni_str!("getFilesDir"),
                jni_sig!("()Ljava/io/File;"),
                &[],
            )?
            .l()?;
        let path = env
            .call_method(
                &files,
                jni_str!("getAbsolutePath"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        Ok(PathBuf::from(java_string(env, path)?))
    }
}

fn cursor_string(
    env: &mut jni::Env<'_>,
    cursor: &JObject<'_>,
    column: i32,
) -> Result<String, AndroidError> {
    let value = env
        .call_method(
            cursor,
            jni_str!("getString"),
            jni_sig!("(I)Ljava/lang/String;"),
            &[JValue::Int(column)],
        )?
        .l()?;
    if value.is_null() {
        Ok(String::new())
    } else {
        java_string(env, value)
    }
}

fn java_string(env: &mut jni::Env<'_>, value: JObject<'_>) -> Result<String, AndroidError> {
    let value = unsafe { JString::from_raw(env, value.into_raw()) };
    Ok(value.try_to_string(env)?)
}

fn content_values_put(
    env: &mut jni::Env<'_>,
    values: &JObject<'_>,
    key: &str,
    value: &str,
) -> Result<(), AndroidError> {
    let key = JString::new(env, key)?;
    let value = JString::new(env, value)?;
    env.call_method(
        values,
        jni_str!("put"),
        jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Landroid/content/ContentValues;"),
        &[JValue::Object(&key), JValue::Object(&value)],
    )?;
    Ok(())
}

fn content_values_put_int(
    env: &mut jni::Env<'_>,
    values: &JObject<'_>,
    key: &str,
    value: i32,
) -> Result<(), AndroidError> {
    let key = JString::new(env, key)?;
    env.call_method(
        values,
        jni_str!("putInt"),
        jni_sig!("(Ljava/lang/String;I)Landroid/content/ContentValues;"),
        &[JValue::Object(&key), JValue::Int(value)],
    )?;
    Ok(())
}

fn copy_file_to_java(
    env: &mut jni::Env<'_>,
    source: &Path,
    output: &JObject<'_>,
) -> Result<(), AndroidError> {
    // Copy first, and close the Java stream on the success *and* failure
    // paths so an error mid-copy does not leak the open OutputStream.
    let result = (|| -> Result<(), AndroidError> {
        let mut input = File::open(source)?;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            let java_bytes = env.byte_array_from_slice(&buffer[..read])?;
            env.call_method(
                output,
                jni_str!("write"),
                jni_sig!("([B)V"),
                &[JValue::Object(&java_bytes)],
            )?;
        }
        env.call_method(output, jni_str!("flush"), jni_sig!("()V"), &[])?;
        Ok(())
    })();
    let _ = env.call_method(output, jni_str!("close"), jni_sig!("()V"), &[]);
    result
}

fn clean_media_value(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "<unknown>" {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn is_supported_audio(mime_type: &str, display_name: &str) -> bool {
    matches!(
        mime_type,
        "audio/mpeg" | "audio/mp3" | "audio/wav" | "audio/x-wav"
    ) || display_name
        .rsplit_once('.')
        .map(|(_, extension)| {
            extension.eq_ignore_ascii_case("mp3") || extension.eq_ignore_ascii_case("wav")
        })
        .unwrap_or(false)
}

fn audio_extension(mime_type: &str, display_name: &str) -> &'static str {
    if mime_type.contains("wav") {
        "wav"
    } else if display_name
        .rsplit_once('.')
        .map(|(_, extension)| extension.eq_ignore_ascii_case("wav"))
        .unwrap_or(false)
    {
        "wav"
    } else {
        "mp3"
    }
}
