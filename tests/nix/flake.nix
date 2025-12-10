{
  description = "test";

  inputs = { };

  outputs = {
    packages.x86_64-linux.test = derivation {
      pname = "test";
      version = "0.0.1";
    };
  };
}
