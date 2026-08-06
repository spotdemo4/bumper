plugins {
    java
    id("com.example.fixture") version "0.0.1"
}

fixtureExtension {
    version = "0.0.1"
}

android {
    defaultConfig {
        versionName = "0.0.1"
        versionCode = 13
    }
}

group = "com.example"
version = "0.0.1" // project version

dependencies {
    implementation("com.example:dependency:0.0.1")
}
