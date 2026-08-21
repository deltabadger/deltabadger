# POST-only switch between asset-first and exchange-first ordering. Turbo prefetches GETs, so the
# flow may only change through an explicit form submission.
class Bots::DcaMultiAssets::OrdersController < ApplicationController
  before_action :authenticate_user!

  include Bots::DcaMultiAssets::WizardSteps

  def create
    session[:bot_config] ||= {}
    session[:bot_config]['flow'] = target_flow
    reset_downstream!
    redirect_to step_path(current_order.first)
  end

  private

  def target_flow
    params[:flow] == 'exchange_first' ? 'exchange_first' : 'asset_first'
  end

  def current_step = current_order.first
end
