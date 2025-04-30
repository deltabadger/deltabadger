class Asset::InferColorFromImageJob < ApplicationJob
  queue_as :default

  def perform(asset)
    asset.infer_color_from_image
  end
end
