[app]

# (string) Title of your application
title = UniFlow Mobile

# (string) Package name
package.name = uniflow_mobile

# (string) Package domain (needed for android packaging)
package.domain = org.uniflow

# (string) Source code where the main.py lives
source.dir = .

# (list) Source files to include (let empty to include all the files)
source.include_exts = py,png,jpg,kv,atlas

# (string) Application version
version = 0.1.0

# (list) Application requirements
# comma separated e.g. requirements = sqlite3,kivy
requirements = python3,kivy,requests,urllib3,certifi

# (str) Custom source folders for requirements
# It may be useful when developing new-style recipes
# requirements.source.kivy = ../../kivy

# (list) Permissions
# INTERNET is required to communicate with the UniFlow daemon API over Wi-Fi
android.permissions = INTERNET

# (int) Target Android API, should be as high as possible.
android.api = 33

# (int) Minimum API your APK will support.
android.minapi = 21

# (str) Android NDK version to use
# android.ndk = 25b

# (bool) Use --private data directory (True) or public (False)
# android.private_storage = True

# (str) Icon of the application
# icon.filename = %(source.dir)s/data/icon.png

# (str) Supported orientations
# Valid values are: landscape, portrait, all
orientation = portrait

# (list) List of service to declare
# services = NAME:ENTRYPOINT_TO_PY,e.g. services = myservice:service.py

# (list) The Android archs to build for
android.archs = arm64-v8a, armeabi-v7a

# (bool) Enable Autobarbecue (True) or not (False)
# android.autobarbecue = False

# (str) Path to a custom whitelist file
# android.whitelist =

# (list) Blacklist patterns to exclude some files
# android.blacklist_src = *__pycache__*

# (str) Android entry point, default is ok for Kivy-based app
# android.entrypoint = org.kivy.android.PythonActivity

[buildozer]
# (int) Log level (0 = error only, 1 = info, 2 = debug and gigabytes of other output)
log_level = 2

# (int) Display warning if buildozer is run as root (0 = false, 1 = true)
warn_on_root = 1
