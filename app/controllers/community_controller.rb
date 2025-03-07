class CommunityController < ApplicationController
  before_action :authenticate_user!

  def access_instructions
    # Just render the HTML view for the turbo frame
  end
end
