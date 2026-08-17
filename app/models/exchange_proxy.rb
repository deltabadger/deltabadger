class ExchangeProxy
  # A proxy stored in AppConfig wins over PROXY_<EXCHANGE> in the environment, so a claimed
  # subscription overrides whatever the container was started with. Neither is required.
  def self.for(exchange)
    exchange_name = exchange.to_s
    AppConfig.get("proxy_#{exchange_name.downcase}").presence ||
      ENV["PROXY_#{exchange_name.upcase}"].presence
  end
end
