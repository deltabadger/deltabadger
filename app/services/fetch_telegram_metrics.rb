class FetchTelegramMetrics < BaseService
  TELEGRAM_TOKEN = ENV.fetch('TELEGRAM_BOT_TOKEN')
  TELEGRAM_GROUP_ID = ENV.fetch('TELEGRAM_GROUP_ID')
  CACHE_KEY = 'telegram_metrics_cache'.freeze

  def call
    return Rails.cache.read(CACHE_KEY) if Rails.cache.exist?(CACHE_KEY)

    url = "https://api.telegram.org/bot#{TELEGRAM_TOKEN}/getChatMembersCount?chat_id=#{TELEGRAM_GROUP_ID}"
    request = Faraday.get(url)
    result = JSON.parse(request.body)

    output_metrics = {
      membersCounter: result.fetch('result')
    }
    Rails.cache.write(CACHE_KEY, output_metrics, expires_in: 1.hour)

    output_metrics
  rescue StandardError => e
    Raven.capture_exception(e)
    { membersCounter: nil }
  end
end
