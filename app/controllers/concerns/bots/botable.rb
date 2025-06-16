module Bots::Botable
  extend ActiveSupport::Concern

  private

  def set_bot
    @bot = current_user.bots.find(params[:bot_id] || params[:id])
    redirect_to bots_path, alert: t('bot.not_found') if @bot.deleted?
  rescue ActiveRecord::RecordNotFound
    redirect_to bots_path, alert: t('bot.not_found')
  end
end
