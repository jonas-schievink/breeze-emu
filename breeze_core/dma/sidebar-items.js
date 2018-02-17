initSidebarItems({"enum":[["TransferMode","Describes how a single DMA unit (max. 4 Bytes) is transferred"]],"fn":[["do_dma","Performs all DMA transactions enabled by the given `channels` bitmask. Returns the number of master cycles spent."],["do_hdma","Performs one H-Blank worth of HDMA transfers (at most 8, if all channels are enabled)."],["init_hdma","Refresh HDMA state for a new frame. This is called at V=0, H~6 and will set up some internal registers for all active channels."]],"struct":[["DmaChannel","The configuration of a DMA channel"]]});