class LegendaryBadgerController < ApplicationController
  before_action :authenticate_user!

  layout 'legendary_badger'

  def show; end
end
