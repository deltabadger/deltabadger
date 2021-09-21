class SitemapController < ApplicationController
  def index
    @host = "#{request.protocol}#{request.host}"
  end
end
