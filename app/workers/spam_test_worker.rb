class SpamTestWorker
  include Sidekiq::Worker

  def perform
    convert_to_satoshis(TransactionsRepository.new.total_btc_bought)
    ProfitableBotsRepository.new.profitable_bots_data(Time.now)
    SpamTestWorker.perform_at(Time.now + 3.second)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
