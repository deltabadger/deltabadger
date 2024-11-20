class CalculatorController < ApplicationController
  before_action :authenticate_user!

  DCA_SIMULATION_ASSETS = %w[btc gspc gdaxi gld ndx usd].freeze

  def show
    set_navigation_session
    @invest_amount = session[:invest_amount]
    @simulation_results = get_simulation_results(invest_amount: @invest_amount)
  end

  private

  def set_navigation_session
    params.permit(:invest_amount)
    session[:invest_amount] = params[:invest_amount]&.to_i || session[:invest_amount] || 1000
  end

  def get_simulation_results(invest_amount:)
    DCA_SIMULATION_ASSETS.map do |asset|
      [asset, DcaSimulation.new(
        asset: asset,
        interval: 1.month,
        amount: invest_amount,
        target_profit: 1_000_000
      ).perform]
    end.to_h
  end
end
