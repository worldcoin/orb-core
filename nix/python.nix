{ lib
, python38
, stdenv
, iris
# , rgb-net # FOSS
}:

let
  packageOverrides = self: super: {
    irisPkg = self.buildPythonPackage {
      name = "iris";
      src = iris;
      IRIS_ENV = "SERVER";
      propagatedBuildInputs = with self; [
        opencv4
        numpy
        onnx
        onnxruntime
        protobuf
        pydantic
        pyyaml
      ];
      patchPhase = ''
        sed -i 's/opencv-python\([^0-9]\+\)4\.7\.0\(\.[0-9]\+\)\?/opencv\14.7.0/' requirements/*.txt pyproject.toml
      '';
      doCheck = false;
    };

    # rgbnetPkg = self.buildPythonPackage {
    #   name = "rgb-net";
    #   src = rgb-net;
    #   RGBNET_ENV = "SERVER";
    #   propagatedBuildInputs = with self; [
    #     opencv4
    #     numpy
    #     onnxruntime
    #   ];
    #   patchPhase = ''
    #     sed -i 's/opencv-python\([^0-9]\+\)4\.7\.0\(\.[0-9]\+\)\?/opencv\14.7.0/' requirements/*.txt pyproject.toml
    #   '';
    #   doCheck = false;
    # };

    opencv4 = super.opencv4.override { enablePython = true; pythonPackages = self; };

    numpy = self.buildPythonPackage rec {
      pname = "numpy";
      version = "1.19.5";
      format = "wheel";
      src = self.fetchPypi ({
        inherit pname version;
        format = "wheel";
        dist = "cp38";
        python = "cp38";
        abi = "cp38";
      } // lib.optionalAttrs (stdenv.targetPlatform.system == "x86_64-linux") {
        platform = "manylinux2010_x86_64";
        hash = "sha256-qdF/K+O0J/uyvOYeWWz1Vdb4pWwiK9LKFIuu615ceDw=";
      } // lib.optionalAttrs (stdenv.targetPlatform.system == "aarch64-linux") {
        platform = "manylinux2014_aarch64";
        hash = "sha256-mav081PD0aDHpfJ2mUgsmHz2Y7Hqwg21m4x7Bh6r1/w=";
      });
      dontFixup = true;
    };

    onnx = self.buildPythonPackage rec {
      pname = "onnx";
      version = "1.10.0";
      format = "wheel";
      src = self.fetchPypi ({
        inherit pname version;
        format = "wheel";
        dist = "cp38";
        python = "cp38";
        abi = "cp38";
      } // lib.optionalAttrs (stdenv.targetPlatform.system == "x86_64-linux") {
        platform = "manylinux_2_12_x86_64.manylinux2010_x86_64";
        hash = "sha256-8SWzeSsgo6Ho3QM36ZXPoZhOYIEFabdMh067d8i2b8c=";
      } // lib.optionalAttrs (stdenv.targetPlatform.system == "aarch64-linux") {
        platform = "manylinux_2_17_aarch64.manylinux2014_aarch64";
        hash = "sha256-9dYbz01OKWOEIyw3bZ9VDdsFRDM17+YIsK4KW/Z8wl8=";
      });
      propagatedBuildInputs = with self; [
        typing-extensions
        protobuf
        numpy
      ];
    };

    onnxruntime = self.buildPythonPackage rec {
      pname = "onnxruntime";
      version = "1.10.0";
      format = "wheel";
      src = self.fetchPypi ({
        inherit pname version;
        format = "wheel";
        dist = "cp38";
        python = "cp38";
        abi = "cp38";
      } // lib.optionalAttrs (stdenv.targetPlatform.system == "x86_64-linux") {
        platform = "manylinux_2_17_x86_64.manylinux2014_x86_64";
        hash = "sha256-ORN2lpH38g4TBw1lv93Z+F+GInT8wXMSw7fT/uivIdg=";
      } // lib.optionalAttrs (stdenv.targetPlatform.system == "aarch64-linux") {
        platform = "manylinux_2_17_aarch64.manylinux2014_aarch64";
        hash = "sha256-YYyExr/3P9bdb88wTrJKgE32wR9RLd6tTMcwdLYAErg=";
      });
      propagatedBuildInputs = with self; [
        flatbuffers
        protobuf
        numpy
      ];
    };

    protobuf = self.buildPythonPackage rec {
      pname = "protobuf";
      version = "3.16.0";
      src = self.fetchPypi {
        inherit pname version;
        hash = "sha256-Io7svt1G11AQ8eD4zjTbzRGuWkDBZan8nTMKWKowKBg=";
      };
      propagatedBuildInputs = with self; [
        six
      ];
    };

    pydantic = self.buildPythonPackage rec {
      pname = "pydantic";
      version = "1.10.10";
      src = self.fetchPypi {
        inherit pname version;
        hash = "sha256-O41b2XiG+etZJgWUIHyfV9zhSm+GnGzuqQGIcV0pkho=";
      };
      propagatedBuildInputs = with self; [
        typing-extensions
      ];
      doCheck = false;
    };

    pyyaml = self.buildPythonPackage rec {
      pname = "PyYAML";
      version = "6.0.1";
      format = "wheel";
      src = self.fetchPypi ({
        inherit pname version;
        format = "wheel";
        dist = "cp38";
        python = "cp38";
        abi = "cp38";
      } // lib.optionalAttrs (stdenv.targetPlatform.system == "x86_64-linux") {
        platform = "manylinux_2_17_x86_64.manylinux2014_x86_64";
        hash = "sha256-fgfL3jkbqWq1jlMv9IA/ecQSk5dRThQTp9x2HM11VzU=";
      } // lib.optionalAttrs (stdenv.targetPlatform.system == "aarch64-linux") {
        platform = "manylinux_2_17_aarch64.manylinux2014_aarch64";
        hash = "sha256-oM0XwV07s/oGl4tOiVjc3G4BdMzqgjADoQbH1NeJmsU=";
      });
    };
  };

in
(python38.override { inherit packageOverrides; }).withPackages (ps: with ps; [
  pip
  irisPkg
  # rgbnetPkg # FOSS
])
