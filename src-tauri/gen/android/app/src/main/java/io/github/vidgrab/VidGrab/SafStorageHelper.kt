package io.github.vidgrab.VidGrab

import android.app.Activity
import android.content.Intent
import android.net.Uri

/**
 * Storage Access Framework (SAF) helper allowing users to pick SD card
 * or public Movies/Music directories on Android 10+.
 */
object SafStorageHelper {
    const val REQUEST_CODE_OPEN_TREE = 9042

    fun launchDirectoryPicker(activity: Activity) {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).apply {
            addFlags(
                Intent.FLAG_GRANT_READ_URI_PERMISSION or
                    Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
                    Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
            )
        }
        activity.startActivityForResult(intent, REQUEST_CODE_OPEN_TREE)
    }

    fun persistTreePermission(activity: Activity, treeUri: Uri) {
        val flags = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        activity.contentResolver.takePersistableUriPermission(treeUri, flags)
    }
}
