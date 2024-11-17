class BarbellBotsController < ApplicationController
  before_action :authenticate_user!
  before_action :set_barbell_bot, only: %i[show]

  def show; end

  private

  def set_barbell_bot
    @barbell_bot = current_user.bots.barbell.last
  end
end
