package app.photosync

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.StatFs
import android.provider.Settings
import android.view.WindowManager
import androidx.activity.result.ActivityResult
import androidx.core.content.ContextCompat
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import org.json.JSONObject

@TauriPlugin(
    permissions = [
        Permission(strings = [Manifest.permission.READ_EXTERNAL_STORAGE], alias = "storage")
    ]
)
class PhotosyncPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun loadSecret(invoke: Invoke) {
        try {
            val key = invoke.getArgs().getString("key")
            val value = SecretsVault.load(activity).optString(key, "")
            invoke.resolve(JSObject().apply {
                put("value", if (value.isEmpty()) JSONObject.NULL else value)
            })
        } catch (e: Exception) {
            invoke.reject("secret vault unavailable", e)
        }
    }

    @Command
    fun saveSecret(invoke: Invoke) {
        try {
            val args = invoke.getArgs()
            SecretsVault.put(activity, args.getString("key"), args.getString("value"))
            invoke.resolve(JSObject())
        } catch (e: Exception) {
            invoke.reject("could not save the secret", e)
        }
    }

    @Command
    fun clearSecrets(invoke: Invoke) {
        try {
            SecretsVault.clear(activity)
            invoke.resolve(JSObject())
        } catch (e: Exception) {
            invoke.reject("could not clear secrets", e)
        }
    }

    @Command
    fun storageInfo(invoke: Invoke) {
        try {
            val path = invoke.getArgs().getString("path")
            val stat = StatFs(path)
            val total = stat.blockCountLong * stat.blockSizeLong
            val free = stat.availableBlocksLong * stat.blockSizeLong
            invoke.resolve(JSObject().apply {
                put("totalBytes", total)
                put("freeBytes", free)
            })
        } catch (e: Exception) {
            invoke.reject("could not read free space for the pool", e)
        }
    }

    @Command
    fun hasAllFilesAccess(invoke: Invoke) {
        invoke.resolve(JSObject().apply { put("granted", allFilesGranted()) })
    }

    @Command
    fun requestAllFilesAccess(invoke: Invoke) {
        if (Build.VERSION.SDK_INT >= 30) {
            val intent = Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION).apply {
                data = Uri.parse("package:${activity.packageName}")
            }
            try {
                startActivityForResult(invoke, intent, "allFilesResult")
            } catch (e: Exception) {
                invoke.reject("could not open the all-files access settings", e)
            }
        } else if (Build.VERSION.SDK_INT >= 23 &&
            ContextCompat.checkSelfPermission(activity, Manifest.permission.READ_EXTERNAL_STORAGE) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissionForAlias("storage", invoke, "storagePermissionResult")
        } else {
            invoke.resolve(JSObject().apply { put("granted", allFilesGranted()) })
        }
    }

    @ActivityCallback
    private fun allFilesResult(invoke: Invoke, result: ActivityResult) {
        invoke.resolve(JSObject().apply { put("granted", allFilesGranted()) })
    }

    @PermissionCallback
    private fun storagePermissionResult(invoke: Invoke) {
        invoke.resolve(JSObject().apply { put("granted", allFilesGranted()) })
    }

    private fun allFilesGranted(): Boolean =
        if (Build.VERSION.SDK_INT >= 30) {
            Environment.isExternalStorageManager()
        } else {
            ContextCompat.checkSelfPermission(activity, Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        }

    @Command
    fun keepScreenOn(invoke: Invoke) {
        val enabled = invoke.getArgs().getBoolean("enabled")
        activity.runOnUiThread {
            if (enabled) {
                activity.window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            } else {
                activity.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
        invoke.resolve(JSObject())
    }

    @Command
    fun startReleaseService(invoke: Invoke) {
        ContextCompat.startForegroundService(
            activity,
            Intent(activity, ReleaseService::class.java)
        )
        invoke.resolve(JSObject())
    }

    @Command
    fun stopReleaseService(invoke: Invoke) {
        activity.stopService(Intent(activity, ReleaseService::class.java))
        invoke.resolve(JSObject())
    }
}
