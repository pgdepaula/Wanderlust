// Synthetic cache-save enumeration client; no game content.
#include <switch.h>
#include <string.h>
static void check(Result r) { if(R_FAILED(r)){svcBreak(0,0,0);svcExitProcess();} }
void __appInit(void) {}
void __appExit(void) {}
int main(void) {
    check(smInitialize());check(fsInitialize());
    for(unsigned iteration=0;iteration<2;iteration++) {
        Service reader={0};
        check(serviceDispatch(fsGetServiceSession(),62,.out_num_objects=1,.out_objects=&reader));
        FsSaveDataInfo entries[2];memset(entries,0x55,sizeof entries);s64 count=-1;
        check(serviceDispatchOut(&reader,0,count,
            .buffer_attrs={SfBufferAttr_HipcMapAlias|SfBufferAttr_Out},
            .buffers={{entries,sizeof entries}}));
        if(count!=0)check(1);
        for(unsigned i=0;i<sizeof entries;i++)if(((const u8*)entries)[i]!=0x55)check(1);
        serviceClose(&reader);
    }
    FsFileSystem save={0};FsSaveDataAttribute attr={0};
    check(fsOpenSaveDataFileSystem(&save,FsSaveDataSpaceId_User,&attr));
    FsFile file={0};FsDir directory={0};FsDirEntryType type;
    if(fsFsOpenFile(&save,"/missing.dat",FsOpenMode_Read,&file)!=0x202)check(1);
    if(fsFsGetEntryType(&save,"/missing.dat",&type)!=0x202)check(1);
    if(fsFsOpenDirectory(&save,"/missing",FsDirOpenMode_ReadDirs,&directory)!=0x202)check(1);
    fsFsClose(&save);
    fsExit();smExit();svcOutputDebugString("FSCACHE_NATIVE_OK",17);svcExitProcess();
}
