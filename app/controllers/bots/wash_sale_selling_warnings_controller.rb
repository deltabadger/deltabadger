# "Got it" on the one-time warning about what wash-sale protection cannot cover while a multi-asset bot
# sells (WashSalePromptable#wash_sale_selling_warning_due?). Recorded here, on acknowledgement, not when
# it is shown: closed any other way, it comes back. The first acknowledgement stands.
class Bots::WashSaleSellingWarningsController < ApplicationController
  before_action :authenticate_user!

  def create
    # The warning is the account's, but it is reached from one of the account's own bots.
    current_user.bots.find(params[:bot_id])
    current_user.update!(wash_sale_selling_warned_at: Time.current) if current_user.wash_sale_selling_warned_at.nil?
    head :no_content
  end
end
