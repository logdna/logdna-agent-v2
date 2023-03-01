cask "mezmo-agent" do
    arch arm: "arm64", intel: "x86"
    version "3.9.0"
    sha256  arm: "66cbfaee8d652fee09b83eef3fc14656336db235ee50bad7e436cf5c63cd5f2c",
            intel: "e70b1373b87a445a2eb0211b58b770be55fb7619261bda534d5926e31834ac35"
  
    url "https://logdna-agent-build-bin.s3.amazonaws.com/logdna-agent-1.0-#{arch}.pkg"
    name "Mezmo Agent"
    desc "Agent streams from log files to your Mezmo account"
    homepage "https://mezmo.com/"
  
    pkg "logdna-agent-1.0-#{arch}.pkg"
  
    uninstall pkgutil:   "com.logdna.logdna-agent",
              launchctl: "com.logdna.logdna-agentd"

    caveats <<~EOS
        To always run logdna-agent in the background, use the command:
        sudo launchctl load -w /Library/LaunchDaemons/com.logdna.logdna-agent.plist
    EOS
  end
