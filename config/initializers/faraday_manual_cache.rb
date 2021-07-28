# Cached API Calls Configure
FaradayManualCache.configure do |config|
  config.memory_store = Rails.cache
end
