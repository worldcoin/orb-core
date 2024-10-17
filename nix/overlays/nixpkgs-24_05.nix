# This overlay gives us access to nixpkgs 24.05
{ inputs, ... }:
final: _prev: {
  nixpkgs-24_05 = import inputs.nixpkgs-24_05 {
    system = final.system;
  };
}
