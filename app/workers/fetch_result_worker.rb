class FetchResultWorker
  include Sidekiq::Worker

  def perform(bot_id, result_parameters, fixing_price)
    FetchOrderResult.call(bot_id, result_parameters, fixing_price)
  rescue StandardError => e
    # prevent job from retrying
  end
end
