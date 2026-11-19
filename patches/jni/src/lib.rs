compile_error!(
    "patches/jni stands in for jni, which only rustls-platform-verifier's Android branch uses: \
     Prebid Server does not target Android. Building for Android needs the real jni and the syn 2 \
     duplicate its jni-macros brings."
);
