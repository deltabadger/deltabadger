require 'rails_helper'

RSpec.describe "PortfolioAnalyzers", type: :request do
  describe "GET /index" do
    it "returns http success" do
      get "/portfolio_analyzer/index"
      expect(response).to have_http_status(:success)
    end
  end

end
