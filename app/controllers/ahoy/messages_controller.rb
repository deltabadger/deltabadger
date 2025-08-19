module Ahoy
  class MessagesController < ApplicationController
    def open
      token = params[:id].to_s
      campaign = params[:c].to_s
      signature = params[:s].to_s

      if AhoyEmail::Utils.signature_verified?(legacy: false, token: token, campaign: campaign, url: '', signature: signature)
        data = {}
        data[:campaign] = campaign if campaign
        data[:token] = token
        data[:url] = ''
        data[:controller] = self
        AhoyEmail::Utils.publish(:open, data)
      end

      send_data Base64.decode64("R0lGODlhAQABAPAAAAAAAAAAACH5BAEAAAAALAAAAAABAAEAAAICRAEAOw=="), type: "image/gif", disposition: "inline"
    end
  end
end
