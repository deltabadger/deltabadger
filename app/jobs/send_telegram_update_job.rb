class SendTelegramUpdateJob < ApplicationJob
  queue_as :default

  def perform
    BotsRepository.new.send_top_bots_update
  end
end
