{ lib
, stdenv
, fetchFromGitHub
, cmake
, pkg-config
, unzip
, zlib
, pcre
, boost
, gflags
, protobuf
, libxcrypt ? null
, buildPackages
, enablePython ? false
, pythonPackages
}:

stdenv.mkDerivation rec {
  pname = "opencv";
  version = "4.7.0";

  src = fetchFromGitHub {
    owner = "opencv";
    repo = "opencv";
    rev = version;
    sha256 = "sha256-jUeGsu8+jzzCnIFbVMCW8DcUeGv/t1yCY/WXyW+uGDI=";
  };

  postConfigure = ''
    [ -e modules/core/version_string.inc ]
    echo '"(build info elided)"' > modules/core/version_string.inc
  '';

  buildInputs = [ zlib pcre boost gflags protobuf libxcrypt ]
    ++ lib.optional enablePython pythonPackages.python;

  propagatedBuildInputs = lib.optional enablePython pythonPackages.numpy;

  nativeBuildInputs = [ cmake pkg-config unzip ]
    ++ lib.optionals enablePython (with pythonPackages; [ pip wheel setuptools ]);

  cmakeFlags = [
    "-DOPENCV_GENERATE_PKGCONFIG=ON"
    "-DWITH_OPENMP=ON"
    "-DBUILD_PROTOBUF=OFF"
    "-DPROTOBUF_UPDATE_FILES=ON"
    "-DProtobuf_PROTOC_EXECUTABLE=${lib.getBin buildPackages.protobuf}/bin/protoc"
    "-DBUILD_TESTS=OFF"
    "-DBUILD_PERF_TESTS=OFF"
    "-DBUILD_LIST=calib3d,imgproc,objdetect,video,highgui,python3"
  ] ++ lib.optionals enablePython [
    "-DOPENCV_SKIP_PYTHON_LOADER=ON"
  ];

  postInstall = ''
    sed -i "s|{exec_prefix}/$out|{exec_prefix}|;s|{prefix}/$out|{prefix}|" \
      "$out/lib/pkgconfig/opencv4.pc"
  '' + lib.optionalString enablePython ''
    cd $NIX_BUILD_TOP/$sourceRoot/modules/python/package
    python -m pip wheel --verbose --no-index --no-deps --no-clean --no-build-isolation --wheel-dir dist .

    cd dist
    python -m pip install ./*.whl --no-index --no-warn-script-location --prefix="$out" --no-cache
    rm -r $out/${pythonPackages.python.sitePackages}/cv2
  '';
}
