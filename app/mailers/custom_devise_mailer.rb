class CustomDeviseMailer < Devise::Mailer
  layout "mailer"

  before_action :set_show_dca_profit

  helper LocalesHelper

  private

  def set_show_dca_profit
    @show_dca_profit = false
  end
end
