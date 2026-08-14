class ExchangeProxy
  # AppConfig (platform-claimed, self-hosted) beats ENV (container-injected, hosted).
  # A hosted container never has the AppConfig rows, so fleet behavior is unchanged.
  def self.for(exchange)
    exchange_name = exchange.to_s
    AppConfig.get("proxy_#{exchange_name.downcase}").presence ||
      ENV["PROXY_#{exchange_name.upcase}"].presence
  end
end
