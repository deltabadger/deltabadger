# frozen_string_literal: true

module Middleware
  class ChromeDevtools
    PATH = "/.well-known/appspecific/com.chrome.devtools.json"

    def initialize(app)
      @app = app
    end

    def call(env)
      return @app.call(env) unless env["PATH_INFO"] == PATH

      body = {
        workspace: {
          root: Rails.root.to_s,
          uuid: Digest::UUID.uuid_v5("822f7bc5-aa31-4b9f-9c14-df23d95578a1", Rails.root.to_s)
        }
      }.to_json

      [200, { "Content-Type" => "application/json" }, [body]]
    end
  end
end
